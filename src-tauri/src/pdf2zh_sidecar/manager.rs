use std::{
    collections::{HashMap, HashSet},
    fs::{self, OpenOptions},
    io::{BufRead, BufReader, Write},
    path::Path,
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use tauri::{AppHandle, Runtime};

use super::{
    paths::{resolve_sidecar_command, Pdf2zhCommand},
    protocol::{SidecarEvent, SidecarRequest},
};

#[derive(Default)]
pub(crate) struct Pdf2zhSidecarState {
    children: Mutex<HashMap<String, Arc<Mutex<Child>>>>,
    active_jobs: Mutex<HashSet<String>>,
}

impl Pdf2zhSidecarState {
    pub(crate) fn try_acquire_job_slot(&self, job_id: &str) -> Result<bool, String> {
        let mut active_jobs = self
            .active_jobs
            .lock()
            .map_err(|_| "PDF translation active-job registry lock was poisoned".to_string())?;
        if active_jobs.contains(job_id) {
            return Ok(true);
        }
        if active_jobs.len() >= max_concurrent_jobs() {
            return Ok(false);
        }
        active_jobs.insert(job_id.to_string());
        Ok(true)
    }

    pub(crate) fn release_job_slot(&self, job_id: &str) {
        if let Ok(mut active_jobs) = self.active_jobs.lock() {
            active_jobs.remove(job_id);
        }
    }

    pub(crate) fn register(&self, job_id: &str, child: Arc<Mutex<Child>>) -> Result<(), String> {
        let mut children = self
            .children
            .lock()
            .map_err(|_| "PDF translation process registry lock was poisoned".to_string())?;
        children.insert(job_id.to_string(), child);
        Ok(())
    }

    pub(crate) fn unregister(&self, job_id: &str) {
        if let Ok(mut children) = self.children.lock() {
            children.remove(job_id);
        }
    }

    pub(crate) fn kill(&self, job_id: &str) -> Result<bool, String> {
        let child = {
            let children = self
                .children
                .lock()
                .map_err(|_| "PDF translation process registry lock was poisoned".to_string())?;
            children.get(job_id).cloned()
        };
        let Some(child) = child else {
            return Ok(false);
        };
        let mut child = child
            .lock()
            .map_err(|_| "PDF translation child process lock was poisoned".to_string())?;
        child
            .kill()
            .map_err(|err| format!("Failed to kill PDF translation sidecar: {err}"))?;
        Ok(true)
    }
}

fn max_concurrent_jobs() -> usize {
    std::env::var("MARKDOWNNOTE_PDF2ZH_MAX_CONCURRENT_JOBS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(1)
}

pub(crate) fn run_request_blocking<R, T, F>(
    app: &AppHandle<R>,
    request: SidecarRequest<T>,
    log_dir: &Path,
    state: Option<&Pdf2zhSidecarState>,
    job_id: Option<&str>,
    mut on_event: F,
) -> Result<SidecarEvent, String>
where
    R: Runtime,
    T: serde::Serialize,
    F: FnMut(&SidecarEvent),
{
    fs::create_dir_all(log_dir).map_err(|err| {
        format!(
            "Failed to create sidecar log dir {}: {err}",
            log_dir.display()
        )
    })?;
    let sidecar_command = resolve_sidecar_command(app)?;
    let request_line = serde_json::to_string(&request)
        .map_err(|err| format!("Failed to encode sidecar request: {err}"))?;
    let mut command = Command::new(sidecar_command.program());
    if let Pdf2zhCommand::Python { worker, .. } = &sidecar_command {
        command.arg("-u").arg(worker);
    }
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("PYTHONUNBUFFERED", "1")
        .env("MARKDOWNNOTE_PDF2ZH_LOG_DIR", log_dir);
    if let Some(dev_source) = dev_pdf2zh_source() {
        command.env("PYTHONPATH", dev_source);
    }

    let mut child = command.spawn().map_err(|err| {
        format!(
            "Failed to start PDF translation sidecar with {}: {err}",
            sidecar_command.display_name()
        )
    })?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "Failed to open sidecar stdin".to_string())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "Failed to open sidecar stdout".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "Failed to open sidecar stderr".to_string())?;

    writeln!(stdin, "{request_line}")
        .map_err(|err| format!("Failed to send sidecar request: {err}"))?;
    drop(stdin);

    let child = Arc::new(Mutex::new(child));
    if let (Some(state), Some(job_id)) = (state, job_id) {
        state.register(job_id, child.clone())?;
    }

    copy_stderr_to_log(stderr, log_dir);

    let mut terminal = None;
    let mut read_error = None;
    let reader = BufReader::new(stdout);
    for line in reader.lines() {
        let line = match line {
            Ok(line) => line,
            Err(err) => {
                read_error = Some(format!("Failed to read sidecar stdout: {err}"));
                break;
            }
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let event: SidecarEvent = match serde_json::from_str(trimmed) {
            Ok(event) => event,
            Err(err) => {
                read_error = Some(format!(
                    "Sidecar emitted non-protocol stdout: {err}; line={trimmed}"
                ));
                if let Ok(mut child) = child.lock() {
                    let _ = child.kill();
                }
                break;
            }
        };
        if event.id != request.id {
            continue;
        }
        on_event(&event);
        if event.terminal_success() || event.terminal_error() {
            terminal = Some(event);
            break;
        }
    }

    let status = loop {
        let maybe_status = {
            let mut child = child
                .lock()
                .map_err(|_| "PDF translation child process lock was poisoned".to_string())?;
            child
                .try_wait()
                .map_err(|err| format!("Failed to poll sidecar process: {err}"))?
        };
        if let Some(status) = maybe_status {
            break status;
        }
        thread::sleep(Duration::from_millis(80));
    };
    if let (Some(state), Some(job_id)) = (state, job_id) {
        state.unregister(job_id);
    }
    if let Some(err) = read_error {
        return Err(err);
    }

    let Some(event) = terminal else {
        return Err(missing_terminal_event_error(&status));
    };
    if event.terminal_error() {
        return Err(if event.message.is_empty() {
            format!(
                "PDF translation sidecar failed status={} code={} retryable={}",
                event.status, event.error_code, event.retryable
            )
        } else {
            format!(
                "{} status={} code={} retryable={}",
                event.message, event.status, event.error_code, event.retryable
            )
        });
    }
    Ok(event)
}

fn missing_terminal_event_error(status: &std::process::ExitStatus) -> String {
    format!("PDF translation sidecar exited without a terminal event (status: {status})")
}

fn copy_stderr_to_log(stderr: impl std::io::Read + Send + 'static, log_dir: &Path) {
    let log_path = log_dir.join("sidecar-stderr.log");
    thread::spawn(move || {
        let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&log_path) else {
            return;
        };
        let timestamp_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis())
            .unwrap_or_default();
        let _ = writeln!(
            file,
            "{{\"ts_ms\":{timestamp_ms},\"stream\":\"stderr-start\"}}"
        );
        let reader = BufReader::new(stderr);
        for line in reader.lines().map_while(Result::ok) {
            let _ = writeln!(file, "{}", redact_log_line(&line));
        }
    });
}

fn redact_log_line(line: &str) -> String {
    line.split_whitespace()
        .map(|token| {
            let lower = token.to_ascii_lowercase();
            if lower.starts_with("sk-")
                || lower.contains("api_key")
                || lower.contains("authorization")
                || lower.contains("token=")
                || lower.contains("key=")
            {
                "[REDACTED]"
            } else {
                token
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn dev_pdf2zh_source() -> Option<std::path::PathBuf> {
    let source = super::paths::dev_project_root()
        .parent()?
        .join("external")
        .join("PDFMathTranslate")
        .join("pdf2zh")
        .join("kernel")
        .join("PDFMathTranslate-next.git");
    source.exists().then_some(source)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stderr_log_redaction_masks_common_secret_tokens() {
        let line = redact_log_line("error api_key=sk-test authorization=Bearer token=abc safe");
        assert!(!line.contains("sk-test"));
        assert!(!line.contains("token=abc"));
        assert!(line.contains("[REDACTED]"));
        assert!(line.contains("safe"));
    }

    #[test]
    fn active_job_slot_defaults_to_single_translation() {
        let state = Pdf2zhSidecarState::default();
        assert!(state.try_acquire_job_slot("job-1").unwrap());
        assert_eq!(
            state.try_acquire_job_slot("job-2").unwrap(),
            max_concurrent_jobs() > 1
        );
        state.release_job_slot("job-1");
        assert!(state.try_acquire_job_slot("job-2").unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn kill_terminates_registered_child_process() {
        let state = Pdf2zhSidecarState::default();
        let child = Command::new("sh")
            .arg("-c")
            .arg("sleep 30")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn child");
        let child = Arc::new(Mutex::new(child));

        state.register("job-1", child.clone()).unwrap();
        assert!(state.kill("job-1").unwrap());
        let status = child.lock().unwrap().wait().unwrap();
        assert!(!status.success());
        state.unregister("job-1");
        assert!(!state.kill("job-1").unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn missing_terminal_event_reports_sidecar_exit_status() {
        use std::os::unix::process::ExitStatusExt;

        let status = std::process::ExitStatus::from_raw(9 << 8);
        let message = missing_terminal_event_error(&status);
        assert!(message.contains("without a terminal event"));
        assert!(message.contains("status:"));
    }
}
