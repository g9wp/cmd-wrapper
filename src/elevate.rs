//! Windows 管理员提权支持（UAC）。
//!
//! 仅在 `cfg(windows)` 目标下编译。提权实现参考 scoop 的 `sudo.ps1`：
//!
//! 1. `run_as_admin = true` 且未提权时，通过 `ShellExecuteExW` 的 `runas` verb
//!    重新启动**自身**触发 UAC，并把父进程 PID、工作目录、配置文件路径通过
//!    **命令行参数**传给提权实例（sudo.ps1 同款：环境变量跨 UAC 不可靠，
//!    命令行参数 100% 可达）；
//! 2. 提权实例在 main 开头（[`init_elevated`]）先 `FreeConsole` 脱离 runas 新建的
//!    控制台，再 `AttachConsole(父进程 PID)` 附加到原始进程所在的终端控制台
//!    （无论 cmd / PowerShell / WezTerm / Windows Terminal 的 ConPTY），
//!    输入输出全部留在当前终端，不闪出新的控制台窗口；
//! 3. 父进程等待提权实例结束，透传其退出码。

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;

/// 提权专用命令行参数：父进程 PID（提权实例用它 AttachConsole 复用终端）
const PARENT_ARG: &str = "--__cmd-wrapper-parent=";
/// 提权专用命令行参数：原始工作目录（UAC 提权后的默认工作目录是 System32）
const CWD_ARG: &str = "--__cmd-wrapper-cwd=";
/// 提权专用命令行参数：配置文件（toml）的绝对路径。
/// 提权实例经 runas 启动时，current_exe() 对硬链接别名可能返回其他名称，
/// 不能用来推导 toml 路径，因此由父进程显式传入。
const CONFIG_ARG: &str = "--__cmd-wrapper-config=";
/// 提权专用参数前缀（main 解析用户参数时需过滤）
pub(crate) const ELEV_ARG_PREFIX: &str = "--__cmd-wrapper-";

/// 判断参数是否为提权专用参数
pub(crate) fn is_elev_arg(arg: &str) -> bool {
    arg.starts_with(ELEV_ARG_PREFIX)
}

/// 确保以管理员身份运行：
/// - 未提权：重新启动自身（触发 UAC），等待提权实例结束后透传其退出码退出；
/// - 已提权：控制台附加与工作目录恢复已由 main 开头的 [`init_elevated`] 完成。
pub fn ensure_elevated() -> anyhow::Result<()> {
    if is_elevated() {
        Ok(())
    } else {
        relaunch_as_admin()
    }
}

/// main 开头调用（须在一切输出之前）：
/// - 提权重启出来的实例：脱离 runas 新建的控制台，附加到父进程（终端）控制台，
///   恢复工作目录，返回 `Some(配置文件路径)`；
/// - 普通实例：返回 `None`。
pub(crate) fn init_elevated() -> Option<std::path::PathBuf> {
    let mut parent_pid = None;
    let mut cwd = None;
    let mut config = None;
    for arg in std::env::args().skip(1) {
        if let Some(v) = arg.strip_prefix(PARENT_ARG) {
            parent_pid = v.parse::<u32>().ok();
        } else if let Some(v) = arg.strip_prefix(CWD_ARG) {
            cwd = Some(v.to_string());
        } else if let Some(v) = arg.strip_prefix(CONFIG_ARG) {
            config = Some(std::path::PathBuf::from(v));
        }
    }
    let Some(parent_pid) = parent_pid else {
        return None; // 普通调用（非提权重启）
    };

    // 参考 sudo.ps1 的 sudo_do：先脱离 runas 新建的控制台，再附加到父进程
    // （终端）的控制台，输入输出回到当前终端
    unsafe {
        use windows_sys::Win32::System::Console::{AttachConsole, FreeConsole};
        FreeConsole();
        AttachConsole(parent_pid);
    }
    // AttachConsole 不会重置标准句柄，显式打开 CONIN$/CONOUT$ 刷新
    ensure_standard_handles();

    // 恢复原始工作目录
    if let Some(cwd) = cwd {
        let _ = std::env::set_current_dir(cwd);
    }

    Some(config.unwrap_or_else(|| {
        std::env::current_exe()
            .map(|p| p.with_extension("toml"))
            .unwrap_or_default()
    }))
}

/// 当前进程是否已持有管理员（提权）令牌
fn is_elevated() -> bool {
    unsafe {
        use windows_sys::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TokenElevation};

        // 使用 NtCurrentProcessToken 伪句柄 (-4) 代替已从 Win32 API 移除的 OpenProcessToken
        let process_token: *mut std::ffi::c_void = (-4isize) as *mut std::ffi::c_void;
        let mut elevation: TOKEN_ELEVATION = std::mem::zeroed();
        let mut size: u32 = 0;
        let ok = GetTokenInformation(
            process_token,
            TokenElevation,
            &mut elevation as *mut TOKEN_ELEVATION as *mut std::ffi::c_void,
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut size,
        );
        ok != 0 && elevation.TokenIsElevated != 0
    }
}

/// 通过 `ShellExecuteExW` 的 `runas` verb 重新启动自身以获取管理员权限。
/// 父进程 PID / 工作目录 / 配置文件路径全部通过命令行参数传递（参考 sudo.ps1）。
fn relaunch_as_admin() -> anyhow::Result<()> {
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_CANCELLED};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, INFINITE, WaitForSingleObject,
    };
    use windows_sys::Win32::UI::Shell::{
        SEE_MASK_FLAG_NO_UI, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE;

    let exe = std::env::current_exe()?;
    let args: Vec<String> = std::env::args().skip(1).collect();

    // 把父进程 PID、工作目录、配置文件路径通过命令行传给提权实例
    let mut all_args = args.clone();
    all_args.push(format!("{PARENT_ARG}{}", std::process::id()));
    if let Ok(cwd) = std::env::current_dir() {
        all_args.push(format!("{CWD_ARG}{}", cwd.display()));
    }
    all_args.push(format!(
        "{CONFIG_ARG}{}",
        exe.with_extension("toml").display()
    ));

    let exe_wide = to_wide(exe.as_os_str());
    let verb_wide = to_wide(OsStr::new("runas"));
    let params_wide = to_wide(OsStr::new(&join_args(&all_args)));

    let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_FLAG_NO_UI;
    info.hwnd = std::ptr::null_mut();
    info.lpVerb = verb_wide.as_ptr();
    info.lpFile = exe_wide.as_ptr();
    info.lpParameters = params_wide.as_ptr();
    info.lpDirectory = std::ptr::null();
    // 隐藏 UAC 启动的提权实例控制台窗口（sudo.ps1 的 windowstyle = 'hidden'）
    info.nShow = SW_HIDE;

    let ok = unsafe { ShellExecuteExW(&mut info) };
    if ok == 0 {
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() == Some(ERROR_CANCELLED as i32) {
            anyhow::bail!("用户取消了管理员权限确认，程序退出");
        }
        anyhow::bail!("无法以管理员身份启动: {err}");
    }

    // 等待提权实例结束，并透传其退出码
    unsafe {
        WaitForSingleObject(info.hProcess, INFINITE);
        let mut code: u32 = 0;
        GetExitCodeProcess(info.hProcess, &mut code);
        CloseHandle(info.hProcess);
        std::process::exit(code as i32);
    }
}

/// 附加到父进程控制台后，标准句柄需要重新打开 CONIN$/CONOUT$：
/// AttachConsole 不会刷新它们（仍指向 runas 新建控制台的已失效句柄）。
fn ensure_standard_handles() {
    unsafe {
        use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE};
        use windows_sys::Win32::Storage::FileSystem::{CreateFileW, OPEN_EXISTING};
        use windows_sys::Win32::System::Console::{
            GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, SetStdHandle,
        };

        let refresh = |std: u32, access: u32, device: &str, share: u32| {
            let h = GetStdHandle(std);
            if h.is_null() || h == INVALID_HANDLE_VALUE {
                let name = to_wide(OsStr::new(device));
                let new = CreateFileW(
                    name.as_ptr(),
                    access,
                    share,
                    std::ptr::null(),
                    OPEN_EXISTING,
                    0,
                    std::ptr::null_mut(),
                );
                if new != INVALID_HANDLE_VALUE && !new.is_null() {
                    SetStdHandle(std, new);
                }
            }
        };
        refresh(STD_INPUT_HANDLE, GENERIC_READ, "CONIN$", 1); // FILE_SHARE_READ
        refresh(STD_OUTPUT_HANDLE, GENERIC_WRITE, "CONOUT$", 2); // FILE_SHARE_WRITE
        refresh(STD_ERROR_HANDLE, GENERIC_WRITE, "CONOUT$", 2); // FILE_SHARE_WRITE
    }
}

fn to_wide(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain(Some(0)).collect()
}

/// 按 Windows CRT 命令行规则拼接参数
fn join_args(args: &[String]) -> String {
    let mut out = String::new();
    for (i, arg) in args.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        out.push_str(&quote_arg(arg));
    }
    out
}

/// 仅对含空白或引号的参数加引号并转义，其余原样保留
fn quote_arg(arg: &str) -> String {
    let needs_quote = arg.chars().any(|c| c.is_whitespace() || c == '"');
    if !needs_quote {
        return arg.to_string();
    }
    let mut out = String::from("\"");
    let mut chars = arg.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                let mut count = 1;
                while matches!(chars.peek(), Some('\\')) {
                    chars.next();
                    count += 1;
                }
                // 反斜杠后紧跟引号或字符串结束时需要加倍，否则会被当作转义符
                let doubled = count
                    * if matches!(chars.peek(), Some('"')) || chars.peek().is_none() {
                        2
                    } else {
                        1
                    };
                out.push_str(&"\\".repeat(doubled));
            }
            '"' => {
                out.push('\\');
                out.push('"');
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quote_plain_arg_unchanged() {
        assert_eq!(quote_arg("--foo"), "--foo");
        assert_eq!(quote_arg("C:\\path\\app.exe"), "C:\\path\\app.exe");
    }

    #[test]
    fn quote_arg_with_space() {
        assert_eq!(quote_arg("has space"), r#""has space""#);
    }

    #[test]
    fn quote_arg_with_embedded_quote() {
        assert_eq!(quote_arg("a\"b"), r#""a\"b""#);
    }

    #[test]
    fn quote_arg_with_inner_backslash() {
        // 无反斜杠转义需求的参数原样保留（不加引号）
        assert_eq!(quote_arg("back\\slash"), "back\\slash");
        // 引号内：反斜杠后跟普通字符（空格）时不加倍
        assert_eq!(quote_arg("back\\ slash"), r#""back\ slash""#);
        // 引号内：反斜杠紧邻引号时需要加倍转义
        assert_eq!(quote_arg("a\\\"b"), r#""a\\\"b""#);
    }

    #[test]
    fn quote_arg_with_trailing_backslash() {
        // 结尾反斜杠（无空格）按原样保留，命令行解析为字面反斜杠
        assert_eq!(quote_arg("trail\\"), "trail\\");
        // 引号内的结尾反斜杠在闭合引号前必须加倍
        assert_eq!(quote_arg("has space\\"), r#""has space\\""#);
    }

    #[test]
    fn join_mixed_args() {
        assert_eq!(join_args(&["a".into(), "b c".into()]), r#"a "b c""#);
        assert_eq!(join_args(&["x".into()]), "x");
        assert_eq!(join_args(&[]), "");
    }

    #[test]
    fn elev_arg_prefix() {
        assert!(is_elev_arg("--__cmd-wrapper-parent=123"));
        assert!(!is_elev_arg("--prog=x"));
    }
}
