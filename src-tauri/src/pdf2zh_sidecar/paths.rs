use std::{
    env, fs,
    path::{Path, PathBuf},
};

use tauri::{AppHandle, Manager, Runtime};

#[derive(Debug, Clone)]
pub(crate) struct Pdf2zhPaths {
    pub(crate) root: PathBuf,
    pub(crate) logs: PathBuf,
    pub(crate) cache: PathBuf,
    pub(crate) artifacts: PathBuf,
    pub(crate) runtime: PathBuf,
    pub(crate) tmp: PathBuf,
}

#[derive(Debug, Clone)]
pub(crate) enum Pdf2zhCommand {
    Python { python: PathBuf, worker: PathBuf },
    Executable { executable: PathBuf },
}

impl Pdf2zhCommand {
    pub(crate) fn program(&self) -> &Path {
        match self {
            Self::Python { python, .. } => python.as_path(),
            Self::Executable { executable } => executable.as_path(),
        }
    }

    pub(crate) fn display_name(&self) -> String {
        match self {
            Self::Python { python, worker } => {
                format!("{} -u {}", python.display(), worker.display())
            }
            Self::Executable { executable } => executable.display().to_string(),
        }
    }
}

impl Pdf2zhPaths {
    pub(crate) fn ensure<R: Runtime>(app: &AppHandle<R>) -> Result<Self, String> {
        let app_data_dir = app
            .path()
            .app_data_dir()
            .map_err(|err| format!("Failed to resolve app data dir: {err}"))?;
        let root = app_data_dir.join("pdf2zh");
        let paths = Self {
            logs: root.join("logs"),
            cache: root.join("cache"),
            artifacts: root.join("artifacts"),
            runtime: root.join("runtime"),
            tmp: root.join("tmp"),
            root,
        };
        for path in [
            &paths.root,
            &paths.logs,
            &paths.cache,
            &paths.artifacts,
            &paths.runtime,
            &paths.tmp,
        ] {
            fs::create_dir_all(path)
                .map_err(|err| format!("Failed to create {}: {err}", path.display()))?;
        }
        Ok(paths)
    }
}

pub(crate) fn resolve_sidecar_command<R: Runtime>(
    app: &AppHandle<R>,
) -> Result<Pdf2zhCommand, String> {
    if let Some(executable) = resolve_explicit_sidecar_executable() {
        return Ok(Pdf2zhCommand::Executable { executable });
    }

    let dev_root = dev_project_root();
    if let Some(python) = dev_python_path(&dev_root) {
        return Ok(Pdf2zhCommand::Python {
            python,
            worker: resolve_worker(app)?,
        });
    }

    if let Ok(resource_dir) = app.path().resource_dir() {
        let executable = frozen_sidecar_path(&resource_dir);
        if executable.exists() {
            return Ok(Pdf2zhCommand::Executable { executable });
        }
        if let Some(python) = bundled_python_path(&resource_dir) {
            return Ok(Pdf2zhCommand::Python {
                python,
                worker: resolve_worker(app)?,
            });
        }
    }

    if cfg!(debug_assertions) {
        return Ok(Pdf2zhCommand::Python {
            python: PathBuf::from("python3"),
            worker: resolve_worker(app)?,
        });
    }

    Err("PDF translation runtime is not bundled. Reinstall MarkdownNote or rebuild the sidecar runtime.".to_string())
}

fn resolve_explicit_sidecar_executable() -> Option<PathBuf> {
    env::var_os("MARKDOWNNOTE_PDF2ZH_SIDECAR")
        .map(PathBuf::from)
        .filter(|path| path.exists())
}

fn dev_python_path(dev_root: &Path) -> Option<PathBuf> {
    let path = if cfg!(windows) {
        dev_root
            .join(".venv-pdf2zh")
            .join("Scripts")
            .join("python.exe")
    } else {
        dev_root.join(".venv-pdf2zh").join("bin").join("python")
    };
    path.exists().then_some(path)
}

fn bundled_python_path(resource_dir: &Path) -> Option<PathBuf> {
    let path = if cfg!(windows) {
        resource_dir
            .join("pdf2zh-sidecar")
            .join("python")
            .join("python.exe")
    } else {
        resource_dir
            .join("pdf2zh-sidecar")
            .join("python")
            .join("bin")
            .join("python3")
    };
    path.exists().then_some(path)
}

fn frozen_sidecar_path(resource_dir: &Path) -> PathBuf {
    resource_dir
        .join("pdf2zh-sidecar")
        .join(frozen_sidecar_filename())
}

fn frozen_sidecar_filename() -> &'static str {
    if cfg!(windows) {
        "pdf2zh-sidecar.exe"
    } else {
        "pdf2zh-sidecar"
    }
}

pub(crate) fn resolve_worker<R: Runtime>(app: &AppHandle<R>) -> Result<PathBuf, String> {
    if let Some(value) = env::var_os("MARKDOWNNOTE_PDF2ZH_WORKER") {
        let path = PathBuf::from(value);
        if path.exists() {
            return Ok(path);
        }
    }

    if let Ok(resource_dir) = app.path().resource_dir() {
        let bundled = resource_dir
            .join("pdf2zh-sidecar")
            .join("worker")
            .join("markdownnote_pdf2zh_worker.py");
        if bundled.exists() {
            return Ok(bundled);
        }
    }

    let dev_worker = dev_project_root()
        .join("resources")
        .join("pdf2zh-sidecar")
        .join("worker")
        .join("markdownnote_pdf2zh_worker.py");
    if dev_worker.exists() {
        return Ok(dev_worker);
    }

    Err("PDF translation worker script is missing".to_string())
}

pub(crate) fn dev_project_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frozen_sidecar_path_uses_platform_filename() {
        let path = frozen_sidecar_path(Path::new("/tmp/markdownnote-resources"));
        if cfg!(windows) {
            assert!(path.ends_with("pdf2zh-sidecar.exe"));
        } else {
            assert!(path.ends_with("pdf2zh-sidecar"));
        }
    }
}
