use anyhow::{Context, Result, bail};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::env;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

// Windows 管理员提权（UAC）支持模块
#[cfg(target_os = "windows")]
mod elevate;

// 创建模式：未找到同名 toml 时生成别名 + 配置
mod create;

#[derive(Debug, Deserialize)]
struct Config {
    prog: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: HashMap<String, String>,
    #[serde(default)]
    shell_quote: bool,
    #[serde(default = "default_io")]
    stdin: String,
    #[serde(default = "default_io")]
    stdout: String,
    #[serde(default = "default_io")]
    stderr: String,
    log_file: Option<LogFile>,
    #[serde(default)]
    run_as_admin: bool,
    #[serde(default = "default_mode")]
    named_args_mode: NamedArgsMode,
    /// 变量替换前缀符号（默认 `$`）。可改为 `%` 等，避免与 bash/fish 等
    /// 脚本自身的 `$var` 语法冲突。
    #[serde(default = "default_var_symbol")]
    var_symbol: String,
}

/// `log_file` 的取值：路径字符串，或布尔简写
/// - `true` → 与配置同名的 `.log` 文件（如 `sf.toml` → `sf.log`）
/// - `false` → 不写日志
#[derive(Debug, Deserialize, Serialize, PartialEq)]
#[serde(untagged)]
enum LogFile {
    Path(String),
    Enabled(bool),
}

impl LogFile {
    /// 解析出实际日志路径；`None` 表示不写日志
    fn resolve(&self, config_path: &Path) -> Option<PathBuf> {
        match self {
            LogFile::Path(p) => Some(PathBuf::from(p)),
            LogFile::Enabled(true) => Some(config_path.with_extension("log")),
            LogFile::Enabled(false) => None,
        }
    }
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum NamedArgsMode {
    All,       // 默认：解析所有 -k v，从位置参数中移除
    BeforeSep, // 仅解析 `--` 之前的命名参数
    Disabled,  // 禁用命名解析：所有输入全进 $argN
    Keep,      // 复制模式：解析为 $key 变量，但位置参数列表中依然保留一份
}

fn default_mode() -> NamedArgsMode {
    NamedArgsMode::All
}

fn default_var_symbol() -> String {
    "$".to_string()
}

fn default_io() -> String {
    "inherit".to_string()
}

fn main() -> Result<()> {
    // Windows: 若本进程是 UAC 提权重启出来的实例，立即脱离 runas 新建的控制台、
    // 附加到父进程（终端）控制台并恢复工作目录（须在一切输出之前完成），
    // 同时据此过滤提权专用参数
    #[cfg(target_os = "windows")]
    let elev_config = elevate::init_elevated();

    // 解析用户输入的参数（提权实例还需过滤提权专用参数）
    #[cfg(target_os = "windows")]
    let user_args: Vec<String> = env::args()
        .skip(1)
        .filter(|a| elev_config.is_none() || !elevate::is_elev_arg(a))
        .collect();
    #[cfg(not(target_os = "windows"))]
    let user_args: Vec<String> = env::args().skip(1).collect();

    // 1. 获取执行程序同名的 toml 文件（提权实例用父进程传入的路径，
    //    因为 runas 启动的进程 current_exe() 对硬链接别名可能返回其他名称）
    let exe_path = env::current_exe()?;
    #[cfg(target_os = "windows")]
    let config_path = match &elev_config {
        Some(p) => p.clone(),
        None => exe_path.with_extension("toml"),
    };
    #[cfg(not(target_os = "windows"))]
    let config_path = exe_path.with_extension("toml");

    // 创建模式：未找到同名 .toml 时，把自身创建为别名并生成对应配置
    if !config_path.exists() {
        return create::run(&exe_path);
    }

    let config_content = std::fs::read_to_string(&config_path)
        .with_context(|| format!("无法读取配置文件: {:?}", config_path))?;
    let config: Config = toml::from_str(&config_content)?;

    // 替换符号不能为空，否则正则 `(\w+)` 会退化为匹配任意单词
    if config.var_symbol.is_empty() {
        bail!("配置项 `var_symbol` 不能为空（默认 \"$\"）");
    }

    // Windows: 按配置以管理员身份运行（触发 UAC 提权后重新启动自身）
    #[cfg(target_os = "windows")]
    if config.run_as_admin {
        elevate::ensure_elevated()?;
    }
    #[cfg(not(target_os = "windows"))]
    if config.run_as_admin {
        eprintln!("警告: `run_as_admin` 仅在 Windows 平台生效，已忽略");
    }

    // 2. 解析用户输入的参数
    let (pos_args, named_args) = parse_user_args(&user_args, &config.named_args_mode);

    // 3. 构建最终参数列表
    let final_args = build_final_args(&config, &pos_args, &named_args);

    // 4. 配置日志（`true` = 与配置同名的 .log；`false` / 未设置 = 不记录）
    if let Some(log_path) = config
        .log_file
        .as_ref()
        .and_then(|l| l.resolve(&config_path))
    {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)?;
        writeln!(
            file,
            "[{:?}] Executing: {} {}",
            chrono::Local::now(),
            config.prog,
            final_args.join("ˑˑ")
        )?;
    }

    // 5. 执行程序
    let mut cmd = Command::new(&config.prog);
    cmd.args(&final_args)
        .envs(&config.env)
        .stdin(parse_stdio(&config.stdin))
        .stdout(parse_stdio(&config.stdout))
        .stderr(parse_stdio(&config.stderr));

    let mut child = cmd
        .spawn()
        .with_context(|| format!("无法启动程序: {}", config.prog))?;
    let status = child.wait()?;

    std::process::exit(status.code().unwrap_or(0));
}

/// 解析用户输入：区分位置参数和命名参数 (--key=val, -key val)
fn parse_user_args(
    args: &[String],
    mode: &NamedArgsMode,
) -> (Vec<String>, HashMap<String, String>) {
    let mut pos = Vec::new();
    let mut named = HashMap::new();
    let mut i = 0;
    let mut reached_separator = false; // 是否遇到了 `--`

    while i < args.len() {
        let arg = &args[i];

        // 处理 POSIX 分隔符 `--`
        if arg == "--" {
            reached_separator = true;
            i += 1;
            continue;
        }

        // 判定当前参数是否应作为“命名参数”解析
        let can_parse_named = match mode {
            NamedArgsMode::Disabled => false,
            NamedArgsMode::BeforeSep => !reached_separator && arg.starts_with('-'),
            _ => arg.starts_with('-'),
        };

        if can_parse_named {
            let key;
            let value;
            let mut step = 1;

            if let Some((k, v)) = arg.split_once('=') {
                key = k.trim_start_matches('-').to_string();
                value = v.to_string();
            } else {
                key = arg.trim_start_matches('-').to_string();
                // 尝试看下一个是不是值 (不是以 - 开头的)
                if i + 1 < args.len() && !args[i + 1].starts_with('-') {
                    value = args[i + 1].clone();
                    step = 2;
                } else {
                    value = "true".to_string();
                }
            }

            named.insert(key, value);

            // 如果是 Keep 模式，解析完还要塞回位置参数里
            if *mode == NamedArgsMode::Keep {
                pos.push(arg.clone());
                if step == 2 {
                    pos.push(args[i + 1].clone());
                }
            }

            i += step;
        } else {
            // 纯位置参数
            pos.push(arg.clone());
            i += 1;
        }
    }
    (pos, named)
}

/// 核心替换逻辑：处理变量、位置参数、切片展开
fn build_final_args(cfg: &Config, pos: &[String], named: &HashMap<String, String>) -> Vec<String> {
    let mut result = Vec::new();
    // 符号需先转义才能拼进正则（如 `$`、`(`、`+` 等）
    let sym = regex::escape(&cfg.var_symbol);
    // 用 `<sym>` 作占位符，避免 `format!` 对正则里的 `{`/`}` 做转义处理
    let re = |pat: &str| Regex::new(&pat.replace("<sym>", &sym)).unwrap();
    let re_var = re(r"<sym>(\w+)|<sym>\((\w+)\)");
    // 多参数模式：`${...args}`（等价 `${...args[..]}`）/ `${...args[start..end]}` —— 每个位置参数各生成一个参数
    let re_multi = re(r"<sym>\{\.\.\.args(?:\[(\d*)\.\.(\d*)\])?\}");
    // 单参数模式：`$(...args[start..end])`，可带 `,<连接符>`（默认空格）—— 先 join 成一个参数
    let re_join = re(r"<sym>\(\.\.\.args(?:\[(\d*)\.\.(\d*)\])?(?:,([^)]*))?\)");

    for tpl in &cfg.args {
        // 多参数模式：区间内每个参数各自生成一个参数（前后缀重复）
        if let Some(caps) = re_multi.captures(tpl) {
            // 区间为 1 基闭区间，与 `$argN` 一致（`[2..4]` = 第 2~4 个参数）
            let start = caps
                .get(1)
                .and_then(|m| m.as_str().parse::<usize>().ok())
                .unwrap_or(1);
            let end = caps
                .get(2)
                .and_then(|m| m.as_str().parse::<usize>().ok())
                .unwrap_or(pos.len());

            let start_idx = start.saturating_sub(1);
            let end_excl = end.min(pos.len());
            // 空区间 / 倒序区间（如 `[5..2]`）时不产生参数，避免切片 panic
            if start_idx < end_excl {
                for item in &pos[start_idx..end_excl] {
                    // 用闭包做字面替换：regex 的字符串替换会把 item 里的 `$` 当作捕获组引用
                    // （`$i` → 空、`$$` → `$`、`$1` → 第 1 个捕获组），必须原样插入外部参数
                    let expanded = re_multi.replace_all(tpl, |_: &regex::Captures| item.as_str());
                    result.push(maybe_quote(&expanded, cfg.shell_quote));
                }
            }
            continue;
        }

        // 单参数模式：先把区间内的参数 join 成一个参数，再整体替换
        if let Some(caps) = re_join.captures(tpl) {
            // 区间为 1 基闭区间，与 `$argN` 一致
            let start = caps
                .get(1)
                .and_then(|m| m.as_str().parse::<usize>().ok())
                .unwrap_or(1);
            let end = caps
                .get(2)
                .and_then(|m| m.as_str().parse::<usize>().ok())
                .unwrap_or(pos.len());
            let sep = caps.get(3).map(|m| m.as_str()).unwrap_or(" ");

            let start_idx = start.saturating_sub(1);
            let end_excl = end.min(pos.len());
            if start_idx < end_excl {
                let joined = pos[start_idx..end_excl].join(sep);
                let expanded = re_join.replace_all(tpl, |_: &regex::Captures| joined.as_str());
                result.push(maybe_quote(&expanded, cfg.shell_quote));
            }
            continue;
        }

        // 处理普通变量替换
        let replaced = re_var.replace_all(tpl, |caps: &regex::Captures| {
            let key = caps.get(1).or(caps.get(2)).map(|m| m.as_str()).unwrap();

            // 优先级：位置参数 (arg1, arg2...) > 命名参数 > TOML环境变量 > 系统环境变量
            if key.starts_with("arg")
                && let Ok(idx) = key[3..].parse::<usize>()
            {
                return pos.get(idx - 1).cloned().unwrap_or_default();
            }

            named
                .get(key)
                .cloned()
                .or_else(|| cfg.env.get(key).cloned())
                .or_else(|| env::var(key).ok())
                .unwrap_or_default()
        });

        if !replaced.is_empty() {
            result.push(maybe_quote(&replaced, cfg.shell_quote));
        }
    }
    result
}

fn maybe_quote(s: &str, should_quote: bool) -> String {
    if should_quote {
        shell_words::quote(s).into_owned()
    } else {
        s.to_string()
    }
}

fn parse_stdio(s: &str) -> Stdio {
    match s.to_lowercase().as_str() {
        "null" => Stdio::null(),
        "piped" => Stdio::piped(),
        _ => Stdio::inherit(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg_with(args: &[&str]) -> Config {
        Config {
            prog: "prog".into(),
            args: args.iter().map(|s| s.to_string()).collect(),
            env: HashMap::new(),
            shell_quote: false,
            stdin: "inherit".into(),
            stdout: "inherit".into(),
            stderr: "inherit".into(),
            log_file: None,
            run_as_admin: false,
            named_args_mode: NamedArgsMode::All,
            var_symbol: "$".into(),
        }
    }

    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn slice_expansion_preserves_dollar_in_external_args() {
        let cfg = cfg_with(&["-lc", "${...args}"]);
        let user = args(&["for i in (seq 3); echo $i; end"]);
        let out = build_final_args(&cfg, &user, &HashMap::new());
        assert_eq!(out, args(&["-lc", "for i in (seq 3); echo $i; end"]));
    }

    #[test]
    fn slice_expansion_dollar_forms_are_literal() {
        // 这些 `$` 形式过去会被 regex 当捕获组引用：`$i`→空、`$$i`→`$i`、`$0`→整个匹配
        let cfg = cfg_with(&["${...args}"]);
        let items = ["$i", "$$i", "$1", "$0", "${x}", "a$b$c"];
        let out = build_final_args(&cfg, &args(&items), &HashMap::new());
        assert_eq!(out, args(&items));
    }

    #[test]
    fn slice_expansion_with_prefix_keeps_dollar() {
        let cfg = cfg_with(&["--a-${...args}"]);
        let out = build_final_args(&cfg, &args(&["$x"]), &HashMap::new());
        assert_eq!(out, args(&["--a-$x"]));
    }

    #[test]
    fn single_arg_mode_joins_with_space() {
        // 无区间：全部位置参数用默认空格连接成一个参数
        let cfg = cfg_with(&["--$(...args)x"]);
        let out = build_final_args(&cfg, &args(&["A1", "A2", "A3"]), &HashMap::new());
        assert_eq!(out, args(&["--A1 A2 A3x"]));
    }

    #[test]
    fn single_arg_mode_range_and_separator() {
        let cfg = cfg_with(&[
            "--$(...args[2..4])x",
            "--$(...args[2..4],,)x",
            "--$(...args[2..4],www)x",
            "--$(...args[2..4],)x",
        ]);
        let out = build_final_args(
            &cfg,
            &args(&["A1", "A2", "A3", "A4", "A5", "A6"]),
            &HashMap::new(),
        );
        assert_eq!(
            out,
            args(&["--A2 A3 A4x", "--A2,A3,A4x", "--A2wwwA3wwwA4x", "--A2A3A4x"])
        );
    }

    #[test]
    fn args_without_range_equals_empty_range() {
        // `...args` 与 `...args[..]` 必须完全等价（多参数/单参数、带连接符均如是）
        let cfg = cfg_with(&[
            "${...args}",
            "${...args[..]}",
            "$(...args)",
            "$(...args[..])",
            "$(...args,,)",
            "$(...args[..],,)",
        ]);
        let out = build_final_args(&cfg, &args(&["A1", "A2", "A3"]), &HashMap::new());
        assert_eq!(
            out,
            args(&[
                "A1", "A2", "A3", "A1", "A2", "A3", "A1 A2 A3", "A1 A2 A3", "A1,A2,A3", "A1,A2,A3",
            ])
        );
    }

    #[test]
    fn range_is_one_based_inclusive() {
        // 与 $argN 一致：args[1..3] == 第 1~3 个参数，缺省界限为 1 / 末尾
        let cfg = cfg_with(&[
            "${...args[1..3]}",
            "$(...args[1..3],,)",
            "${...args[3..3]}",
            "${...args[..]}",
            "${...args[2..]}",
            "${...args[..2]}",
        ]);
        let out = build_final_args(&cfg, &args(&["A1", "A2", "A3", "A4"]), &HashMap::new());
        assert_eq!(
            out,
            args(&[
                "A1", "A2", "A3", "A1,A2,A3", "A3", "A1", "A2", "A3", "A4", "A2", "A3", "A4", "A1",
                "A2",
            ])
        );
    }

    #[test]
    fn single_arg_mode_keeps_dollar_and_handles_empty_range() {
        // join 后的内容里的 `$` 必须原样保留
        let cfg = cfg_with(&["$(...args)"]);
        let out = build_final_args(&cfg, &args(&["$X", "$Y"]), &HashMap::new());
        assert_eq!(out, args(&["$X $Y"]));

        // 无参数 / 空区间：不产生参数（与多参数模式一致）
        let cfg = cfg_with(&[
            "--$(...args)x",
            "--$(...args[2..2])x",
            "--$(...args[5..2])x",
        ]);
        let out = build_final_args(&cfg, &[], &HashMap::new());
        assert!(out.is_empty());
    }

    #[test]
    fn single_arg_mode_with_custom_symbol() {
        let mut cfg = cfg_with(&["%(...args[2..3],,)"]);
        cfg.var_symbol = "%".into();
        let out = build_final_args(&cfg, &args(&["A1", "A2", "A3"]), &HashMap::new());
        assert_eq!(out, args(&["A2,A3"]));
    }

    #[test]
    fn var_expansion_inserts_value_literally() {
        let cfg = cfg_with(&["$arg1"]);
        let out = build_final_args(&cfg, &args(&["$HOME/$x"]), &HashMap::new());
        assert_eq!(out, args(&["$HOME/$x"]));
    }

    #[test]
    fn custom_symbol_substitutes_and_keeps_dollar_literal() {
        let mut cfg = cfg_with(&["$HOME", "%arg1", "%{...args}"]);
        cfg.var_symbol = "%".into();
        let out = build_final_args(&cfg, &args(&["A", "$B"]), &HashMap::new());
        // `$HOME` 不再是占位符（原样传递）；`%arg1` 替换为 A；`%{...args}` 逐个展开
        assert_eq!(out, args(&["$HOME", "A", "A", "$B"]));
    }

    #[test]
    fn custom_symbol_named_and_env() {
        let mut cfg = cfg_with(&["%user", "%VERSION"]);
        cfg.var_symbol = "%".into();
        cfg.env.insert("VERSION".into(), "1.0".into());
        let mut named = HashMap::new();
        named.insert("user".to_string(), "admin".to_string());
        let out = build_final_args(&cfg, &[], &named);
        assert_eq!(out, args(&["admin", "1.0"]));
    }

    #[test]
    fn custom_symbol_with_regex_metachar() {
        let mut cfg = cfg_with(&["+arg1", "+{...args[2..2]}"]);
        cfg.var_symbol = "+".into();
        let out = build_final_args(&cfg, &args(&["X", "Y"]), &HashMap::new());
        assert_eq!(out, args(&["X", "Y"]));
    }

    #[test]
    fn log_file_true_resolves_to_sibling_log() {
        let cfg: Config = toml::from_str("prog = \"p\"\nlog_file = true\n").unwrap();
        let resolved = cfg
            .log_file
            .as_ref()
            .unwrap()
            .resolve(Path::new("G:/dev/bin/sf.toml"));
        assert_eq!(resolved, Some(PathBuf::from("G:/dev/bin/sf.log")));
    }

    #[test]
    fn log_file_path_and_false() {
        let cfg: Config = toml::from_str("prog = \"p\"\nlog_file = \"C:/logs/x.log\"\n").unwrap();
        assert_eq!(
            cfg.log_file.as_ref().unwrap().resolve(Path::new("a.toml")),
            Some(PathBuf::from("C:/logs/x.log"))
        );

        let cfg: Config = toml::from_str("prog = \"p\"\nlog_file = false\n").unwrap();
        assert_eq!(
            cfg.log_file.as_ref().unwrap().resolve(Path::new("a.toml")),
            None
        );

        let cfg: Config = toml::from_str("prog = \"p\"\n").unwrap();
        assert!(cfg.log_file.is_none());
    }
}
