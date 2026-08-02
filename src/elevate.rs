//! Windows 管理员提权支持（UAC）。
//!
//! 仅在 `cfg(windows)` 目标下编译。核心思路：
//! 未提权时通过 `ShellExecuteExW` 的 `runas` verb 重新启动自身以触发 UAC，
//! 提权后的实例再附加到原始进程的控制台、恢复原始工作目录，然后正常执行。

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;

/// 传给提权后实例的环境变量：原始进程 PID（用于附加控制台）
const PARENT_PID_VAR: &str = "CMD_WRAPPER_PARENT_PID";
/// 传给提权后实例的环境变量：原始工作目录（UAC 提权后的默认工作目录是 System32）
const PARENT_CWD_VAR: &str = "CMD_WRAPPER_PARENT_CWD";

/// 确保以管理员身份运行：
/// - 未提权：重新启动自身（触发 UAC），等待提权实例结束后透传其退出码退出；
/// - 已提权：恢复原始控制台与工作目录，继续正常执行。
pub fn ensure_elevated() -> anyhow::Result<()> {
    if is_elevated() {
        restore_parent_context();
        Ok(())
    } else {
        relaunch_as_admin()
    }
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

/// 通过 `ShellExecuteExW` 的 `runas` verb 重新启动自身以获取管理员权限
fn relaunch_as_admin() -> anyhow::Result<()> {
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_CANCELLED};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, INFINITE, WaitForSingleObject,
    };
    use windows_sys::Win32::UI::Shell::{
        SEE_MASK_FLAG_NO_UI, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    // 把原始工作目录与 PID 通过环境变量传给提权后的实例
    if let Ok(cwd) = std::env::current_dir() {
        unsafe { std::env::set_var(PARENT_CWD_VAR, cwd) };
    }
    unsafe { std::env::set_var(PARENT_PID_VAR, std::process::id().to_string()) };

    let exe = std::env::current_exe()?;
    let args: Vec<String> = std::env::args().skip(1).collect();

    let exe_wide = to_wide(exe.as_os_str());
    let verb_wide = to_wide(OsStr::new("runas"));
    let params_wide = to_wide(OsStr::new(&join_args(&args)));

    let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_FLAG_NO_UI;
    info.hwnd = std::ptr::null_mut();
    info.lpVerb = verb_wide.as_ptr();
    info.lpFile = exe_wide.as_ptr();
    info.lpParameters = params_wide.as_ptr();
    info.lpDirectory = std::ptr::null();
    info.nShow = SW_SHOWNORMAL;

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

/// 提权重启出来的实例调用：附加到原进程的控制台、恢复原工作目录
fn restore_parent_context() {
    if let Ok(cwd) = std::env::var(PARENT_CWD_VAR) {
        let _ = std::env::set_current_dir(cwd);
    }
    if let Ok(pid) = std::env::var(PARENT_PID_VAR) {
        if let Ok(pid) = pid.parse::<u32>() {
            attach_console(pid);
        }
    }
    // 清理标记变量，避免泄漏给目标程序
    unsafe {
        std::env::remove_var(PARENT_PID_VAR);
        std::env::remove_var(PARENT_CWD_VAR);
    }
}

/// 附加到指定进程的控制台（与 gsudo 同款方案），保证输入输出仍在原终端
fn attach_console(parent_pid: u32) {
    unsafe {
        use windows_sys::Win32::System::Console::{
            AllocConsole, AttachConsole, FreeConsole, GetStdHandle, STD_ERROR_HANDLE,
            STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, SetStdHandle,
        };
        // 先脱离 UAC 默认新建的控制台，再附加到原进程的控制台
        FreeConsole();
        if AttachConsole(parent_pid) != 0 {
            // AttachConsole 后刷新标准句柄，确保后续输出进入原终端
            SetStdHandle(STD_INPUT_HANDLE, GetStdHandle(STD_INPUT_HANDLE));
            SetStdHandle(STD_OUTPUT_HANDLE, GetStdHandle(STD_OUTPUT_HANDLE));
            SetStdHandle(STD_ERROR_HANDLE, GetStdHandle(STD_ERROR_HANDLE));
        } else {
            // 附加失败（如原进程无控制台）时兜底：重新分配一个可见控制台，
            // 并同样刷新标准句柄，避免 FreeConsole 后句柄失效导致输出丢失
            AllocConsole();
            SetStdHandle(STD_INPUT_HANDLE, GetStdHandle(STD_INPUT_HANDLE));
            SetStdHandle(STD_OUTPUT_HANDLE, GetStdHandle(STD_OUTPUT_HANDLE));
            SetStdHandle(STD_ERROR_HANDLE, GetStdHandle(STD_ERROR_HANDLE));
        }
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
}
