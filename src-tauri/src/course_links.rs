//! Open explicit web links with the OS browser handler, without a command shell.

pub(crate) fn normalize_web_url(input: &str) -> Result<String, String> {
    let value = input.trim();
    if value.is_empty() || value.chars().any(char::is_control) {
        return Err("链接为空或包含无效控制字符".to_string());
    }
    let url = reqwest::Url::parse(value).map_err(|_| "网页链接格式不正确，请使用完整的 http:// 或 https:// 地址".to_string())?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err("仅支持打开 http:// 或 https:// 网页链接".to_string());
    }
    Ok(url.to_string())
}

pub(crate) fn open_url(input: &str) -> Result<(), String> {
    let url = normalize_web_url(input)?;
    #[cfg(target_os = "windows")]
    {
        use std::ffi::c_void;
        #[link(name = "shell32")]
        extern "system" {
            fn ShellExecuteW(window: *mut c_void, operation: *const u16, file: *const u16,
                parameters: *const u16, directory: *const u16, show: i32) -> *mut c_void;
        }
        let operation: Vec<u16> = "open".encode_utf16().chain(std::iter::once(0)).collect();
        let target: Vec<u16> = url.encode_utf16().chain(std::iter::once(0)).collect();
        // Both buffers are NUL-terminated and live until the call returns.
        // No parameters or executable command text are passed to the shell.
        let result = unsafe { ShellExecuteW(std::ptr::null_mut(), operation.as_ptr(), target.as_ptr(),
            std::ptr::null(), std::ptr::null(), 1 /* SW_SHOWNORMAL */) } as isize;
        if result <= 32 {
            return Err(format!("无法调用系统浏览器（系统错误 {result}），请检查默认浏览器设置"));
        }
        return Ok(());
    }
    #[cfg(target_os = "macos")]
    let command = "open";
    #[cfg(all(unix, not(target_os = "macos")))]
    let command = "xdg-open";
    #[cfg(unix)]
    {
        std::process::Command::new(command).arg(&url).spawn()
            .map_err(|error| format!("无法调用系统浏览器：{error}"))?;
        return Ok(());
    }
    #[allow(unreachable_code)]
    Err("当前平台不支持打开外部网页".to_string())
}

#[cfg(test)]
mod tests {
    use super::normalize_web_url;

    #[test]
    fn accepts_https_query_and_fragment_without_shell_interpretation() {
        assert_eq!(normalize_web_url(" https://example.com/a?x=1&y=100%25#part ").unwrap(), "https://example.com/a?x=1&y=100%25#part");
        assert!(normalize_web_url("https://example.com/?q=\"a&b\"").unwrap().contains("%22"));
    }
    #[test]
    fn accepts_uppercase_scheme_and_encodes_unicode() {
        assert_eq!(normalize_web_url("HTTPS://EXAMPLE.COM").unwrap(), "https://example.com/");
        assert!(normalize_web_url("https://example.com/课程").unwrap().contains('%'));
    }
    #[test]
    fn rejects_non_web_protocols_and_relative_files() {
        for input in ["javascript:alert(1)", "file:///C:/secret.txt", "cmd:calc", "mailto:a@example.com", "other.md", "#title"] {
            assert!(normalize_web_url(input).is_err(), "accepted {input}");
        }
    }
    #[test]
    fn rejects_empty_malformed_and_control_characters() {
        for input in ["", " ", "https://", "http://?a=b", "https://example.com/\0bad", "https://example.com/a\nb", "https://example.com/a\tb"] {
            assert!(normalize_web_url(input).is_err(), "accepted {input:?}");
        }
    }
}
