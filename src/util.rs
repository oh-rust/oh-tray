//! 通用工具函数：路径归一化、运行期唯一 id、日志重定向。

use std::sync::atomic::{AtomicUsize, Ordering};

/// 将 `c:/abc/def` -> `c:\abc\def`
/// 而 `taskkill /F /IM tvcms.exe` 中的 `/F` 和 `/IM` 的 `/` 则跳过（前面紧跟空格时原样保留）。
pub fn normalize_path(path: &str) -> String {
    let mut result = String::with_capacity(path.len());

    for (i, c) in path.char_indices() {
        if c == '/' && i > 0 && path.as_bytes()[i - 1] == b' ' {
            result.push(c);
        } else if c == '/' || c == '\\' {
            result.push(std::path::MAIN_SEPARATOR);
        } else {
            result.push(c);
        }
    }

    result
}

pub fn default_true() -> bool {
    true
}

static NEXT_ID: AtomicUsize = AtomicUsize::new(1);

/// 生成一个进程运行期内的唯一 id，用于关联菜单项与运行中的子进程。
pub fn next_runtime_id() -> usize {
    NEXT_ID.fetch_add(1, Ordering::SeqCst)
}

/// 将标准错误输出重定向到指定文件（用于脱离控制台的后台运行）。
#[cfg(unix)]
pub fn redirect_stderr(file: &std::fs::File) {
    use std::os::unix::io::AsRawFd;
    unsafe {
        libc::dup2(file.as_raw_fd(), libc::STDERR_FILENO);
    }
}

#[cfg(windows)]
pub fn redirect_stderr(file: &std::fs::File) {
    use std::os::windows::io::AsRawHandle;
    unsafe {
        windows_sys::Win32::System::Console::SetStdHandle(
            windows_sys::Win32::System::Console::STD_ERROR_HANDLE,
            file.as_raw_handle() as *mut std::ffi::c_void,
        );
    }
}
