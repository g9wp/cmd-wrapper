//! 创建模式：未找到同名 `.toml` 配置文件时，把当前程序复制/硬链接/符号链接为别名，
//! 并把命令行选项序列化为别名对应的 `.toml` 配置。
//!
//! 例如：
//! ```text
//! cmd-wrapper --hardlink C:/msys64/usr/local/bin/sudo_fish.exe \
//!     --program=C:/msys64/usr/bin/fish.exe \
//!     --args=-lc --args="${...args}" \
//!     --quote_args=true --run_as_admin=true
//! ```
//! 会创建 `C:/msys64/usr/local/bin/sudo_fish.exe`（硬链接）和 `C:/msys64/usr/local/bin/sudo_fish.toml`。

use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::collections::HashMap;
use std::path::Path;

use super::LogFile;

#[derive(Debug, Clone, Copy, PartialEq)]
enum LinkMode {
    Hard,
    Symlink,
    Copy,
}

/// 待写入 TOML 的配置（仅包含显式指定的选项）
#[derive(Debug, Serialize)]
struct CreateOptions {
    program: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    args: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    symbol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    env: Option<HashMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    quote_args: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stdin: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stdout: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stderr: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    log_file: Option<LogFile>,
    #[serde(skip_serializing_if = "Option::is_none")]
    run_as_admin: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    options_mode: Option<String>,
    /// 仅用于 `--full`：把全部选项写入配置（未启用的以注释补充），不参与序列化
    #[serde(skip)]
    full: bool,
}

/// 创建模式的入口（由 main 在未找到同名 toml 时调用）
pub fn run(exe_path: &Path) -> Result<()> {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    if raw.iter().any(|a| a == "-h" || a == "--help") {
        print_usage();
        return Ok(());
    }
    let (mode, target, opts) = parse_args_from(&raw)?;

    let target = Path::new(&target);
    let config_path = target.with_extension("toml");
    if target.exists() {
        bail!("目标已存在: {}", target.display());
    }
    if config_path.exists() {
        bail!("配置已存在: {}", config_path.display());
    }
    if let Some(parent) = target.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("无法创建目录: {}", parent.display()))?;
    }

    // 1. 创建别名（复制 / 硬链接 / 符号链接）
    match mode {
        LinkMode::Copy => {
            std::fs::copy(exe_path, target)
                .with_context(|| format!("复制 {} 到 {}", exe_path.display(), target.display()))?;
        }
        LinkMode::Hard => {
            std::fs::hard_link(exe_path, target).with_context(|| {
                format!(
                    "创建硬链接失败（硬链接要求源与目标在同一磁盘卷上）: {}",
                    target.display()
                )
            })?;
        }
        LinkMode::Symlink => {
            create_symlink(exe_path, target).with_context(|| {
                format!(
                    "创建符号链接失败（Windows 下需要管理员权限或开发者模式）: {}",
                    target.display()
                )
            })?;
        }
    }
    println!("已创建别名: {} ({})", target.display(), mode_name(mode));

    // 2. 生成别名对应的 toml（`--full` 时附带全部未启用选项的注释模板）
    let toml_str = if opts.full {
        render_full(&opts)
    } else {
        toml::to_string_pretty(&opts)?
    };
    std::fs::write(&config_path, &toml_str)
        .with_context(|| format!("无法写入配置: {}", config_path.display()))?;
    println!("已创建配置: {}", config_path.display());

    Ok(())
}

fn parse_args_from(args: &[String]) -> Result<(LinkMode, String, CreateOptions)> {
    let mut mode = LinkMode::Copy;
    let mut target: Option<String> = None;
    let mut opts = CreateOptions {
        program: String::new(),
        args: None,
        symbol: None,
        env: None,
        quote_args: None,
        stdin: None,
        stdout: None,
        stderr: None,
        log_file: None,
        run_as_admin: None,
        options_mode: None,
        full: false,
    };

    for arg in args {
        match arg.as_str() {
            "-l" | "--hardlink" => {
                mode = LinkMode::Hard;
                continue;
            }
            "-s" | "--symlink" | "--softlink" => {
                mode = LinkMode::Symlink;
                continue;
            }
            "-c" | "--copy" => {
                mode = LinkMode::Copy;
                continue;
            }
            "--full" | "--template" => {
                opts.full = true;
                continue;
            }
            _ => {}
        }

        if let Some((key, value)) = arg.split_once('=') {
            match key {
                "--program" => opts.program = value.to_string(),
                "--args" => opts
                    .args
                    .get_or_insert_with(Vec::new)
                    .push(value.to_string()),
                "--symbol" => {
                    if value.is_empty() {
                        bail!("--symbol 不能为空");
                    }
                    opts.symbol = Some(value.to_string());
                }
                "--env" => {
                    let (k, v) = value
                        .split_once('=')
                        .with_context(|| format!("--env 格式应为 KEY=VALUE，收到: {value}"))?;
                    opts.env
                        .get_or_insert_with(HashMap::new)
                        .insert(k.to_string(), v.to_string());
                }
                "--quote_args" => opts.quote_args = Some(parse_bool(value, "--quote_args")?),
                "--stdin" => opts.stdin = Some(value.to_string()),
                "--stdout" => opts.stdout = Some(value.to_string()),
                "--stderr" => opts.stderr = Some(value.to_string()),
                "--log_file" => {
                    opts.log_file = Some(match value.to_ascii_lowercase().as_str() {
                        "true" | "yes" | "on" | "1" => LogFile::Enabled(true),
                        "false" | "no" | "off" | "0" => LogFile::Enabled(false),
                        _ => LogFile::Path(value.to_string()),
                    });
                }
                "--run_as_admin" => opts.run_as_admin = Some(parse_bool(value, "--run_as_admin")?),
                "--options_mode" => opts.options_mode = Some(value.to_string()),
                _ => bail!("未知选项: {arg}"),
            }
            continue;
        }

        if arg.starts_with('-') {
            bail!("未知选项: {arg}");
        }

        if target.is_some() {
            bail!("多余的位置参数: {arg}");
        }
        target = Some(arg.clone());
    }

    if opts.program.is_empty() {
        bail!("缺少必填选项 --program=<程序>");
    }
    let target = target.ok_or_else(|| anyhow::anyhow!("缺少目标别名路径（第一个位置参数）"))?;
    Ok((mode, target, opts))
}

fn parse_bool(s: &str, opt: &str) -> Result<bool> {
    match s.to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Ok(true),
        "false" | "0" | "no" | "off" => Ok(false),
        _ => bail!("{opt} 需要 true/false，收到: {s}"),
    }
}

fn mode_name(m: LinkMode) -> &'static str {
    match m {
        LinkMode::Hard => "硬链接",
        LinkMode::Symlink => "符号链接",
        LinkMode::Copy => "复制",
    }
}

/// 生成包含全部选项的配置模板：已启用的选项正常写入，
/// 未启用的按默认值以注释补充。
fn render_full(opts: &CreateOptions) -> String {
    let mut out = String::new();
    out.push_str(&format!("program = {}\n", toml_str(&opts.program)));
    if let Some(args) = &opts.args {
        let items: Vec<String> = args.iter().map(|a| toml_str(a)).collect();
        out.push_str(&format!("args = [{}]\n", items.join(", ")));
    }
    if let Some(v) = &opts.symbol {
        out.push_str(&format!("symbol = {}\n", toml_str(v)));
    }
    if let Some(env) = &opts.env {
        out.push_str(&format!("env = {}\n", toml_inline_table(env)));
    }
    if let Some(v) = opts.quote_args {
        out.push_str(&format!("quote_args = {v}\n"));
    }
    if let Some(v) = &opts.stdin {
        out.push_str(&format!("stdin = {}\n", toml_str(v)));
    }
    if let Some(v) = &opts.stdout {
        out.push_str(&format!("stdout = {}\n", toml_str(v)));
    }
    if let Some(v) = &opts.stderr {
        out.push_str(&format!("stderr = {}\n", toml_str(v)));
    }
    if let Some(v) = &opts.log_file {
        let repr = match v {
            LogFile::Path(p) => toml_str(p),
            LogFile::Enabled(b) => b.to_string(),
        };
        out.push_str(&format!("log_file = {repr}\n"));
    }
    if let Some(v) = opts.run_as_admin {
        out.push_str(&format!("run_as_admin = {v}\n"));
    }
    if let Some(v) = &opts.options_mode {
        out.push_str(&format!("options_mode = {}\n", toml_str(v)));
    }

    // 未启用的选项：按默认值注释补充
    let mut commented = String::new();
    if opts.args.is_none() {
        commented.push_str("# args = []  # 参数模板，可重复指定\n");
    }
    if opts.symbol.is_none() {
        commented.push_str("# symbol = \"$\"  # 变量替换前缀符号\n");
    }
    if opts.env.is_none() {
        commented.push_str("# env = {}  # 额外环境变量，如 { KEY = \"VALUE\" }\n");
    }
    if opts.quote_args.is_none() {
        commented.push_str("# quote_args = false  # 对替换后的变量做引号转义\n");
    }
    if opts.stdin.is_none() {
        commented.push_str("# stdin = \"inherit\"  # inherit / null / piped\n");
    }
    if opts.stdout.is_none() {
        commented.push_str("# stdout = \"inherit\"  # inherit / null / piped\n");
    }
    if opts.stderr.is_none() {
        commented.push_str("# stderr = \"inherit\"  # inherit / null / piped\n");
    }
    if opts.log_file.is_none() {
        commented.push_str("# log_file = false  # true = 与程序同名的 .log；或写具体路径\n");
    }
    if opts.run_as_admin.is_none() {
        commented.push_str("# run_as_admin = false  # Windows: 以管理员身份运行\n");
    }
    if opts.options_mode.is_none() {
        commented.push_str("# options_mode = \"keep\"  # parse / before_dashdash / off / keep\n");
    }
    if !commented.is_empty() {
        out.push_str("\n# --- 未启用的选项（默认值，取消注释即可启用）---\n");
        out.push_str(&commented);
    }
    out
}

/// TOML 基本字符串（转义反斜杠、引号与控制字符）
fn toml_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

/// TOML 内联表（按键排序，保证输出稳定）
fn toml_inline_table(map: &HashMap<String, String>) -> String {
    if map.is_empty() {
        return "{}".to_string();
    }
    let mut entries: Vec<(&String, &String)> = map.iter().collect();
    entries.sort_by(|a, b| a.0.cmp(b.0));
    let body: Vec<String> = entries
        .iter()
        .map(|(k, v)| format!("{} = {}", toml_str(k), toml_str(v)))
        .collect();
    format!("{{ {} }}", body.join(", "))
}

#[cfg(windows)]
fn create_symlink(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_file(src, dst)
}

#[cfg(unix)]
fn create_symlink(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(src, dst)
}

#[cfg(not(any(windows, unix)))]
fn create_symlink(_src: &Path, _dst: &Path) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "当前平台不支持符号链接",
    ))
}

fn print_usage() {
    println!(
        "Cmd-Wrapper 创建模式（未找到同名 .toml 时自动启用）\n\
         \n\
         用法: {} <目标别名路径> [选项]\n\
         \n\
         别名创建方式:\n\
         \x20 -l, --hardlink    以硬链接创建别名（需与源在同一磁盘卷）\n\
         \x20 -s, --symlink     以符号链接创建别名（Windows 需管理员权限或开发者模式）\n\
         \x20 -c, --copy        以复制方式创建别名（默认）\n\
         \n\
         TOML 选项（均可转换为命令行参数）:\n\
         \x20 --program=<程序>         必填，真正要跑的程序\n\
         \x20 --args=<参数>            参数模板，可重复指定，写几次加几个\n\
         \x20 --symbol=<符号>          占位符开头符号（默认 \"$\"）\n\
         \x20 --env=<KEY=VALUE>        环境变量，可重复指定\n\
         \x20 --quote_args=<BOOL>      是否自动给替换内容加引号\n\
         \x20 --stdin / --stdout / --stderr=<模式>   inherit / null / piped\n\
         \x20 --log_file=<路径|true>   日志路径；true = 同名的 .log\n\
         \x20 --options_mode=<模式>    带名字参数的解析方式: parse / before_dashdash / off / keep\n\
         \x20 --run_as_admin=<BOOL>    Windows：以管理员身份运行\n\
         \x20 --full, --template       非 TOML 项：把全部选项都写进配置（没启用的注释掉）\n\
         \n\
         示例:\n\
         \x20 cmd-wrapper --hardlink C:/msys64/usr/local/bin/sudo_fish.exe --program=C:/msys64/usr/bin/fish.exe \\\n\
         \x20   --args=-lc --args=\"${{...args}}\" --quote_args=true --run_as_admin=true",
        std::env::args()
            .next()
            .map(|p| {
                Path::new(&p)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or(p)
            })
            .unwrap_or_else(|| "cmd-wrapper".to_string())
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<(LinkMode, String, CreateOptions)> {
        parse_args_from(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn hardlink_example() {
        let (mode, target, opts) = parse(&[
            "--hardlink",
            "C:/msys64/usr/local/bin/sudo_fish.exe",
            "--program=C:/msys64/usr/bin/fish.exe",
            "--args=-lc",
            "--args=${...args}",
            "--symbol=%",
            "--quote_args=true",
            "--run_as_admin=true",
        ])
        .unwrap();
        assert_eq!(mode, LinkMode::Hard);
        assert_eq!(target, "C:/msys64/usr/local/bin/sudo_fish.exe");
        assert_eq!(opts.program, "C:/msys64/usr/bin/fish.exe");
        assert_eq!(
            opts.args.as_deref(),
            Some(&["-lc".to_string(), "${...args}".to_string()][..])
        );
        assert_eq!(opts.quote_args, Some(true));
        assert_eq!(opts.run_as_admin, Some(true));
        assert_eq!(opts.symbol.as_deref(), Some("%"));
        assert!(opts.env.is_none());
        assert!(opts.log_file.is_none());
    }

    #[test]
    fn copy_is_default_and_symlink_aliases() {
        let (mode, _, _) = parse(&["t.exe", "--program=p"]).unwrap();
        assert_eq!(mode, LinkMode::Copy);
        let (mode, _, _) = parse(&["-s", "t.exe", "--program=p"]).unwrap();
        assert_eq!(mode, LinkMode::Symlink);
        let (mode, _, _) = parse(&["--softlink", "t.exe", "--program=p"]).unwrap();
        assert_eq!(mode, LinkMode::Symlink);
    }

    #[test]
    fn env_and_all_options() {
        let (_, _, opts) = parse(&[
            "t.exe",
            "--program=p",
            "--env=A=1",
            "--env=B=x=y",
            "--stdin=null",
            "--stdout=piped",
            "--stderr=inherit",
            "--log_file=C:/logs/x.log",
            "--options_mode=keep",
        ])
        .unwrap();
        let env = opts.env.unwrap();
        assert_eq!(env.get("A"), Some(&"1".to_string()));
        assert_eq!(env.get("B"), Some(&"x=y".to_string())); // 值中可含 =
        assert_eq!(opts.stdin.as_deref(), Some("null"));
        assert_eq!(opts.stdout.as_deref(), Some("piped"));
        assert_eq!(opts.stderr.as_deref(), Some("inherit"));
        assert_eq!(
            opts.log_file,
            Some(LogFile::Path("C:/logs/x.log".to_string()))
        );
        assert_eq!(opts.options_mode.as_deref(), Some("keep"));
    }

    #[test]
    fn missing_prog_is_error() {
        assert!(parse(&["t.exe"]).is_err());
        assert!(parse(&[]).is_err());
    }

    #[test]
    fn unknown_option_is_error() {
        assert!(parse(&["t.exe", "--program=p", "--bogus=1"]).is_err());
        assert!(parse(&["t.exe", "--program=p", "-x"]).is_err());
    }

    #[test]
    fn bad_bool_is_error() {
        assert!(parse(&["t.exe", "--program=p", "--run_as_admin=maybe"]).is_err());
    }

    #[test]
    fn empty_symbol_is_error() {
        assert!(parse(&["t.exe", "--program=p", "--symbol="]).is_err());
    }

    #[test]
    fn template_is_alias_for_full() {
        let (_, _, opts) = parse(&["t.exe", "--program=p", "--template"]).unwrap();
        assert!(opts.full);
    }

    #[test]
    fn cli_option_names() {
        let (_, _, opts) = parse(&[
            "t.exe",
            "--program=p",
            "--symbol=%",
            "--quote_args=true",
            "--options_mode=off",
        ])
        .unwrap();
        assert_eq!(opts.program, "p");
        assert_eq!(opts.symbol.as_deref(), Some("%"));
        assert_eq!(opts.quote_args, Some(true));
        assert_eq!(opts.options_mode.as_deref(), Some("off"));
    }

    #[test]
    fn log_file_true_shorthand() {
        let (_, _, opts) = parse(&["t.exe", "--program=p", "--log_file=true"]).unwrap();
        assert_eq!(opts.log_file, Some(LogFile::Enabled(true)));
        let (_, _, opts) = parse(&["t.exe", "--program=p", "--log_file=false"]).unwrap();
        assert_eq!(opts.log_file, Some(LogFile::Enabled(false)));
    }

    #[test]
    fn full_output_comments_unset_options() {
        let (_, _, opts) = parse(&["t.exe", "--program=p", "--args=-lc", "--full"]).unwrap();
        assert!(opts.full);
        let s = render_full(&opts);
        assert!(s.contains("program = \"p\""));
        assert!(s.contains("args = [\"-lc\"]"));
        assert!(s.contains("# quote_args = false"));
        assert!(s.contains("# symbol = \"$\""));
        assert!(s.contains("# log_file = false"));
        assert!(!s.lines().any(|l| l.starts_with("quote_args")));
        // 生成的文件必须能被正常解析
        let cfg: crate::Config = toml::from_str(&s).unwrap();
        assert_eq!(cfg.program, "p");
        assert_eq!(cfg.args, vec!["-lc".to_string()]);
        assert!(cfg.log_file.is_none());
    }

    #[test]
    fn full_output_keeps_enabled_options_active() {
        let (_, _, opts) = parse(&[
            "t.exe",
            "--program=p",
            "--quote_args=true",
            "--log_file=true",
            "--env=A=1",
            "--symbol=%",
            "--full",
        ])
        .unwrap();
        let s = render_full(&opts);
        assert!(s.contains("quote_args = true"));
        assert!(s.contains("log_file = true"));
        assert!(s.contains("env = { \"A\" = \"1\" }"));
        assert!(s.contains("symbol = \"%\""));
        assert!(!s.contains("# quote_args"));
        assert!(!s.contains("# log_file"));
        let cfg: crate::Config = toml::from_str(&s).unwrap();
        assert!(cfg.quote_args);
        assert_eq!(cfg.symbol, "%");
        assert_eq!(cfg.env.get("A"), Some(&"1".to_string()));
        assert_eq!(cfg.log_file, Some(LogFile::Enabled(true)));
    }

    #[test]
    fn toml_serialization_shape() {
        let (_, _, opts) = parse(&[
            "C:/msys64/usr/local/bin/sudo_fish.exe",
            "--program=C:/msys64/usr/bin/fish.exe",
            "--args=-lc",
            "--args=${...args}",
            "--symbol=%",
            "--quote_args=true",
            "--run_as_admin=true",
        ])
        .unwrap();
        let s = toml::to_string_pretty(&opts).unwrap();
        assert!(s.contains("program = \"C:/msys64/usr/bin/fish.exe\""));
        assert!(s.contains("\"-lc\""));
        assert!(s.contains("\"${...args}\""));
        assert!(s.contains("symbol = \"%\""));
        assert!(s.contains("quote_args = true"));
        assert!(s.contains("run_as_admin = true"));
        // 未指定的字段不写入
        assert!(!s.contains("stdin"));
        assert!(!s.contains("log_file"));
        assert!(!s.contains("[env]"));
    }
}
