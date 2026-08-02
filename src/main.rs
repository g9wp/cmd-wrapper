use anyhow::{Context, Result};
use regex::Regex;
use serde::Deserialize;
use std::collections::HashMap;
use std::env;
use std::fs::OpenOptions;
use std::io::Write;
use std::process::{Command, Stdio};

// Windows 管理员提权（UAC）支持模块
#[cfg(target_os = "windows")]
mod elevate;

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
    log_file: Option<String>,
    #[serde(default)]
    run_as_admin: bool,
    #[serde(default = "default_mode")]
    named_args_mode: NamedArgsMode,
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

fn default_io() -> String {
    "inherit".to_string()
}

fn main() -> Result<()> {
    // 1. 获取执行程序同名的 toml 文件
    let exe_path = env::current_exe()?;
    let config_path = exe_path.with_extension("toml");
    let config_content = std::fs::read_to_string(&config_path)
        .with_context(|| format!("无法读取配置文件: {:?}", config_path))?;
    let config: Config = toml::from_str(&config_content)?;

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
    let user_args: Vec<String> = env::args().skip(1).collect();
    let (pos_args, named_args) = parse_user_args(&user_args, &config.named_args_mode);

    // 3. 构建最终参数列表
    let final_args = build_final_args(&config, &pos_args, &named_args);

    // 4. 配置日志
    if let Some(log_path) = &config.log_file {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_path)?;
        writeln!(
            file,
            "[{:?}] Executing: {} {}",
            chrono::Local::now(),
            config.prog,
            final_args.join(" ")
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
    let re_var = Regex::new(r"\$(\w+)|\$\((\w+)\)").unwrap();
    let re_slice = Regex::new(r"\$\(\.\.\.args\[(\d*)\.\.(\d*)\]\)|\$\(\.\.\.args\)").unwrap();

    for tpl in &cfg.args {
        // 处理展开语法 $...args[start..end]
        if let Some(caps) = re_slice.captures(tpl) {
            let start = caps
                .get(1)
                .and_then(|m| m.as_str().parse::<usize>().ok())
                .unwrap_or(0);
            let end = caps
                .get(2)
                .and_then(|m| m.as_str().parse::<usize>().ok())
                .unwrap_or(pos.len());

            if start < pos.len() {
                let actual_end = end.min(pos.len());
                for item in &pos[start..actual_end] {
                    result.push(maybe_quote(&re_slice.replace(tpl, item), cfg.shell_quote));
                }
            }
            continue;
        }

        // 处理普通变量替换
        let replaced = re_var.replace_all(tpl, |caps: &regex::Captures| {
            let key = caps.get(1).or(caps.get(2)).map(|m| m.as_str()).unwrap();

            // 优先级：位置参数 (arg1, arg2...) > 命名参数 > TOML环境变量 > 系统环境变量
            if key.starts_with("arg") {
                if let Ok(idx) = key[3..].parse::<usize>() {
                    return pos.get(idx - 1).cloned().unwrap_or_default();
                }
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
