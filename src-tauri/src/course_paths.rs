//! 课程展示版：把运行数据固定在项目目录内。
//!
//! 桌面应用默认把自己的数据写进系统的应用数据目录（Windows 上是
//! `%APPDATA%\<identifier>`）。课程展示的要求是"数据都在项目里"，因此这里提供
//! 两条显式开关：
//!
//! - `MARKDOWNNOTE_PROJECT_DATA_DIR`：开发/兼容启动的数据根目录；
//! - `MARKDOWNNOTE_PROJECT_LOG_DIR`：日志目录，未设置时用 `<数据根>/logs`。
//!
//! 设置项目数据目录后，下列内容全部落在其中：
//!
//! - `markdownnote.sqlite`：新建 SQLite 主数据库（含 WAL/SHM 附属文件）
//! - `notes/`：默认的 Markdown 镜像目录（`vault::default_vault_dir`）
//! - `backups/`：默认的备份快照目录（`resolved_backup_dir`）
//! - `webview2/`：WebView2 用户数据目录（显式传给
//!   `WebviewWindowBuilder::data_directory`，不是"建个空目录就算数"）
//! - `logs/`：应用日志（`tauri-plugin-log` 的 `TargetKind::Folder`）、诊断日志
//!
//! 目录解析、创建或写权限检查失败时返回错误，由调用方终止启动；**不会**悄悄回落到
//! 系统目录。发布版由 exe 同目录的固定标记确定数据位置，可直接双击；
//! 无标记且未显式配置时拒绝启动，不迁移或删除已有数据。

use std::path::{Path, PathBuf};

use std::io::Write;

/// 项目数据根目录开关。相对路径按当前工作目录解析。
pub(crate) const PROJECT_DATA_DIR_ENV: &str = "MARKDOWNNOTE_PROJECT_DATA_DIR";

/// 项目日志目录开关。相对路径按当前工作目录解析；未设置时用 `<数据根>/logs`。
pub(crate) const PROJECT_LOG_DIR_ENV: &str = "MARKDOWNNOTE_PROJECT_LOG_DIR";

fn project_env(name: &str) -> Option<std::ffi::OsString> {
    std::env::var_os(name)
}

fn project_env_text(name: &str) -> Option<String> {
    project_env(name).and_then(|value| value.into_string().ok())
}

/// WebView2 在启动时读取的用户数据目录环境变量，由 WebView2 Loader 解析。
const WEBVIEW2_USER_DATA_ENV: &str = "WEBVIEW2_USER_DATA_FOLDER";

pub(crate) const PACKAGE_MARKER: &str = ".markdownnote-app";
const PACKAGE_ID: &str = "com.course.markdownnote";

/// Only an explicitly marked distribution directory is eligible for exe-local data.
fn packaged_paths(directory: &Path) -> Result<(PathBuf, PathBuf), String> {
    let marker = std::fs::read_to_string(directory.join(PACKAGE_MARKER))
        .map_err(|_| "找不到完整程序标记，请完整解压或安装后打开 MarkdownNote.exe".to_string())?;
    if marker.trim() != PACKAGE_ID {
        return Err("程序目录标记不匹配，请使用完整的课程版程序".to_string());
    }
    Ok((directory.join("运行数据"), directory.join("日志")))
}

/// Prefer the canonical database; recognize one existing note database by schema.
/// Ambiguous, unreadable or unrelated files must never produce an empty library.
pub(crate) fn database_path(directory: &Path) -> Result<PathBuf, String> {
    let current = directory.join("markdownnote.sqlite");
    if current.exists() || !directory.exists() { return Ok(current); }
    let mut databases = Vec::new();
    for entry in std::fs::read_dir(directory).map_err(|e| format!("无法检查笔记库目录：{e}"))? {
        let entry = entry.map_err(|e| format!("无法读取笔记库目录：{e}"))?;
        let path = entry.path();
        if path.extension().and_then(|v| v.to_str()).map(|v| v.eq_ignore_ascii_case("sqlite")) != Some(true) {
            continue;
        }
        if !entry.file_type().map_err(|e| format!("无法检查数据库文件：{e}"))?.is_file() {
            return Err("笔记库不能使用链接或目录形式的 SQLite 文件".into());
        }
        let conn = rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|e| format!("已有数据库无法读取，请先检查文件或恢复备份：{e}"))?;
        let columns: i64 = conn.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('documents') WHERE name IN ('id','title','body_md','content_type')",
            [], |row| row.get(0)).map_err(|e| format!("无法核验已有数据库：{e}"))?;
        let tables: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('collections','app_settings','note_links')",
            [], |row| row.get(0)).map_err(|e| format!("无法核验已有数据库：{e}"))?;
        if columns != 4 || tables != 3 {
            return Err("目录内有无法识别的 SQLite 文件；为避免误选或创建空库，已停止启动".into());
        }
        databases.push(path);
    }
    match databases.len() {
        0 => Ok(current),
        1 => Ok(databases.remove(0)),
        _ => Err("发现多个既有笔记库，请备份后确认要打开的数据库；未合并或改写文件".into()),
    }
}

/// The handle remains held for the complete desktop session. The installer uses
/// the same lock file, so it cannot replace/delete a running application's files.
pub(crate) fn acquire_session_lock(directory: &Path) -> Result<Option<std::fs::File>, String> {
    ensure_writable_dir(directory)?;
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(0);
    }
    match options.open(directory.join(".MarkdownNote.running.lock")) {
        Ok(file) => Ok(Some(file)),
        #[cfg(windows)]
        Err(error) if matches!(error.raw_os_error(), Some(32) | Some(33)) => Ok(None),
        Err(error) => Err(format!("无法锁定笔记目录 {}：{error}", directory.display())),
    }
}

/// Called before any runtime or WebView is created. Explicit legacy launchers
/// keep their data location; direct exe starts use only their marked own folder.
pub(crate) fn start_desktop_session() -> Result<Option<std::fs::File>, String> {
    if project_env(PROJECT_DATA_DIR_ENV).is_none() {
        let exe = std::env::current_exe().map_err(|error| format!("无法定位程序：{error}"))?;
        let directory = exe.parent().ok_or("无法定位程序目录")?;
        let (data, logs) = packaged_paths(directory)?;
        std::env::set_var(PROJECT_DATA_DIR_ENV, data);
        std::env::set_var(PROJECT_LOG_DIR_ENV, logs);
    }
    let root = required_project_root(project_env_text(PROJECT_DATA_DIR_ENV))?;
    acquire_session_lock(&root)
}

/// 把环境变量的原始值解析成绝对目录：未设置或全为空白时返回 `None`；相对路径按
/// 当前工作目录展开，保证后续写入都基于绝对路径。
fn resolve_env_dir(raw: Option<String>) -> Option<PathBuf> {
    let raw = raw?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let path = PathBuf::from(trimmed);
    if path.is_absolute() {
        Some(path)
    } else {
        std::env::current_dir().ok().map(|cwd| cwd.join(path))
    }
}

/// 当前进程的项目数据目录；未设置时 None，由启动门槛拒绝运行。
pub(crate) fn project_data_root() -> Option<PathBuf> {
    resolve_env_dir(project_env_text(PROJECT_DATA_DIR_ENV))
}

/// 项目目录下的固定子目录；启动成功后项目根目录必已设置。
pub(crate) fn project_subdir(name: &str) -> Option<PathBuf> {
    project_data_root().map(|root| root.join(name))
}

/// 目录必须真实可用：创建、写入探针文件、删除探针。任一环节失败都返回错误，
/// 由调用方明确终止启动，避免"看起来在项目里、其实写到了系统目录"。
pub(crate) fn ensure_writable_dir(dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir)
        .map_err(|err| format!("无法创建目录 {}：{err}", dir.display()))?;
    // create_new prevents overwriting any existing file, including an old probe.
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|err| format!("系统时钟无效：{err}"))?
        .as_nanos();
    let probe = dir.join(format!(".markdownnote-write-test-{}-{nonce}", std::process::id()));
    let mut file = std::fs::OpenOptions::new()
        .write(true).create_new(true).open(&probe)
        .map_err(|err| format!("目录不可写 {}：{err}", dir.display()))?;
    let written = file.write_all(b"ok");
    drop(file);
    let removed = std::fs::remove_file(&probe);
    written.map_err(|err| format!("目录写入失败 {}：{err}", dir.display()))?;
    removed.map_err(|err| format!("无法删除目录探针 {}：{err}", probe.display()))?;
    Ok(())
}

/// 课程版不允许绕过启动器后静默写入系统目录。
fn required_project_root(raw: Option<String>) -> Result<PathBuf, String> {
    resolve_env_dir(raw).ok_or_else(||
        format!("未设置 {PROJECT_DATA_DIR_ENV}；请使用项目内的启动器打开笔记工具"))
}

pub(crate) fn resolve_app_data_dir(_app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let root = required_project_root(project_env_text(PROJECT_DATA_DIR_ENV))?;
    ensure_writable_dir(&root)?;
    Ok(root)
}

fn prepare_project_dirs(root: &Path) -> Result<(PathBuf, PathBuf), String> {
    ensure_writable_dir(root)?;
    let webview = root.join("webview2");
    let temporary = root.join("temp");
    ensure_writable_dir(&webview)?;
    ensure_writable_dir(&temporary)?;
    Ok((webview, temporary))
}

/// WebView 必须使用的项目目录；未设置或不可写时返回错误。
pub(crate) fn webview_data_dir() -> Result<Option<PathBuf>, String> {
    let root = required_project_root(project_env_text(PROJECT_DATA_DIR_ENV))?;
    let dir = root.join("webview2");
    ensure_writable_dir(&dir)?;
    Ok(Some(dir))
}

/// 日志目录：优先 `MARKDOWNNOTE_PROJECT_LOG_DIR`，否则 `<数据根>/logs`；
/// 未设置项目数据目录时返回 `None`（沿用系统日志目录的原版行为）。
fn resolve_log_dir(raw: Option<String>, project_root: Option<PathBuf>) -> Option<PathBuf> {
    if let Some(dir) = resolve_env_dir(raw) {
        return Some(dir);
    }
    project_root.map(|root| root.join("logs"))
}

/// 当前进程的日志目录；未设置项目数据目录时返回 `None`。
pub(crate) fn log_dir() -> Option<PathBuf> {
    resolve_log_dir(project_env_text(PROJECT_LOG_DIR_ENV), project_data_root())
}

/// 必须在建立运行时、插件和窗口前准备项目目录。
pub(crate) fn prepare_webview_data_dir() -> Result<Option<PathBuf>, String> {
    let root = required_project_root(project_env_text(PROJECT_DATA_DIR_ENV))?;
    let (webview, temporary) = prepare_project_dirs(&root)?;
    let logs = log_dir().ok_or("无法解析项目日志目录")?;
    ensure_writable_dir(&logs)?;
    std::env::set_var(WEBVIEW2_USER_DATA_ENV, &webview);
    std::env::set_var("TEMP", &temporary);
    std::env::set_var("TMP", &temporary);
    Ok(Some(webview))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 相对路径必须展开成绝对路径，否则数据目录会随启动时的当前目录漂移。
    #[test]
    fn relative_project_dir_is_resolved_against_the_working_directory() {
        let root = resolve_env_dir(Some("runtime-data".to_string())).expect("应当解析出项目目录");
        assert!(root.is_absolute(), "相对路径必须展开：{}", root.display());
        assert!(root.ends_with("runtime-data"));
    }

    /// 绝对路径原样使用，不再拼接工作目录。
    #[test]
    fn absolute_project_dir_is_used_as_is() {
        let raw = if cfg!(windows) {
            r"D:\course\data"
        } else {
            "/tmp/course/data"
        };
        let root = resolve_env_dir(Some(raw.to_string())).expect("绝对路径");
        assert_eq!(root, PathBuf::from(raw));
    }

    /// 空白值等于"未设置"，不能让程序把当前目录当成数据目录。
    #[test]
    fn blank_project_dir_is_not_a_valid_path() {
        assert!(resolve_env_dir(None).is_none());
        assert!(resolve_env_dir(Some(String::new())).is_none());
        assert!(resolve_env_dir(Some("   ".to_string())).is_none());
    }

    /// 日志目录：显式开关优先，其次 `<数据根>/logs`，都没有则为 None。
    #[test]
    fn log_dir_prefers_explicit_setting_then_project_subfolder() {
        let root = PathBuf::from(if cfg!(windows) { r"D:\p" } else { "/tmp/p" });
        let explicit = if cfg!(windows) {
            r"D:\p\logs-explicit"
        } else {
            "/tmp/p/logs-explicit"
        };
        assert_eq!(
            resolve_log_dir(Some(explicit.to_string()), Some(root.clone())),
            Some(PathBuf::from(explicit))
        );
        assert_eq!(
            resolve_log_dir(None, Some(root.clone())),
            Some(root.join("logs"))
        );
        assert_eq!(resolve_log_dir(Some("   ".to_string()), None), None);
        assert_eq!(resolve_log_dir(None, None), None);
    }

    /// 可写目录检查必须真的写一次；探针文件不能留下。
    #[test]
    fn writable_dir_check_creates_and_probes_the_directory() {
        let dir = std::env::temp_dir().join(format!(
            "markdownnote-path-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        ensure_writable_dir(&dir).expect("应当可以创建并写入");
        assert!(dir.is_dir());
        assert!(
            !dir.join(".markdownnote-write-test").exists(),
            "探针文件必须被删除"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod course_local_path_tests {
    use super::*;

    fn test_root() -> PathBuf {
        std::env::temp_dir().join(format!("course-local-path-{}-{}", std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()))
    }

    fn existing_database(path: &Path) {
        let conn = rusqlite::Connection::open(path).unwrap();
        conn.execute_batch("CREATE TABLE documents(id TEXT, title TEXT, body_md TEXT, content_type TEXT);
            CREATE TABLE collections(id TEXT); CREATE TABLE app_settings(key TEXT); CREATE TABLE note_links(id TEXT);
            INSERT INTO documents VALUES('n1','保留笔记','原有正文','note');").unwrap();
    }

    #[test]
    fn database_new_library_uses_canonical_filename() {
        let root = test_root();
        assert_eq!(database_path(&root).unwrap(), root.join("markdownnote.sqlite"));
        assert!(!root.exists());
    }

    #[test]
    fn database_existing_library_is_detected_without_renaming_or_writing() {
        let root = test_root(); std::fs::create_dir_all(&root).unwrap();
        let original = root.join("previous-notes.sqlite"); existing_database(&original);
        let bytes = std::fs::read(&original).unwrap();
        assert_eq!(database_path(&root).unwrap(), original);
        assert_eq!(std::fs::read(&original).unwrap(), bytes);
        assert!(!root.join("markdownnote.sqlite").exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn database_canonical_name_takes_priority_over_other_libraries() {
        let root = test_root(); std::fs::create_dir_all(&root).unwrap();
        existing_database(&root.join("previous-notes.sqlite"));
        let current = root.join("markdownnote.sqlite"); existing_database(&current);
        assert_eq!(database_path(&root).unwrap(), current);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn database_ambiguous_or_invalid_files_fail_without_creating_a_library() {
        let root = test_root(); std::fs::create_dir_all(&root).unwrap();
        existing_database(&root.join("first.sqlite")); existing_database(&root.join("second.sqlite"));
        assert!(database_path(&root).is_err());
        std::fs::remove_file(root.join("second.sqlite")).unwrap();
        std::fs::write(root.join("broken.sqlite"), b"not a database").unwrap();
        assert!(database_path(&root).is_err());
        assert!(!root.join("markdownnote.sqlite").exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_or_blank_project_root_rejects_startup() {
        for raw in [None, Some(String::new()), Some("   ".to_string())] {
            assert!(required_project_root(raw).unwrap_err().contains(PROJECT_DATA_DIR_ENV));
        }
    }

    #[test]
    fn startup_directories_are_created_under_explicit_root() {
        let root = test_root();
        let (webview, temp) = prepare_project_dirs(&root).unwrap();
        assert_eq!(webview, root.join("webview2"));
        assert_eq!(temp, root.join("temp"));
        assert!(webview.is_dir() && temp.is_dir());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn file_in_place_of_root_causes_explicit_error() {
        let root = test_root();
        std::fs::write(&root, b"preserve me").unwrap();
        assert!(prepare_project_dirs(&root).is_err());
        assert_eq!(std::fs::read(&root).unwrap(), b"preserve me");
        std::fs::remove_file(root).unwrap();
    }

    #[test]
    fn directory_probe_preserves_existing_files_and_cleans_up() {
        let root = test_root();
        std::fs::create_dir_all(&root).unwrap();
        let existing = root.join(".markdownnote-write-test");
        std::fs::write(&existing, b"existing user file").unwrap();
        ensure_writable_dir(&root).unwrap();
        assert_eq!(std::fs::read(&existing).unwrap(), b"existing user file");
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unmarked_or_wrong_package_cannot_choose_a_data_directory() {
        let root = test_root();
        std::fs::create_dir_all(&root).unwrap();
        assert!(packaged_paths(&root).is_err());
        std::fs::write(root.join(PACKAGE_MARKER), "other.application").unwrap();
        assert!(packaged_paths(&root).is_err());
        assert!(!root.join("运行数据").exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn marked_package_resolves_both_data_and_logs_inside_its_own_directory() {
        let root = test_root();
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join(PACKAGE_MARKER), format!("{PACKAGE_ID}\n")).unwrap();
        let (data, logs) = packaged_paths(&root).unwrap();
        assert_eq!(data, root.join("运行数据"));
        assert_eq!(logs, root.join("日志"));
        assert!(!data.exists(), "path resolution must not create runtime files");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn second_session_is_rejected_and_lock_is_released_on_exit() {
        let root = test_root();
        let first = acquire_session_lock(&root).unwrap().unwrap();
        assert!(acquire_session_lock(&root).unwrap().is_none());
        drop(first);
        let next = acquire_session_lock(&root).unwrap().unwrap();
        drop(next);
        std::fs::remove_dir_all(root).unwrap();
    }
}
