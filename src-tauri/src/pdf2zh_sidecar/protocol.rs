use serde::{Deserialize, Serialize};

pub(crate) const PROTOCOL_VERSION: &str = "markdownnote-pdf2zh-v1";

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SidecarRequest<T> {
    pub(crate) id: String,
    pub(crate) protocol_version: String,
    pub(crate) kind: String,
    pub(crate) payload: T,
}

impl<T> SidecarRequest<T> {
    pub(crate) fn new(id: String, kind: impl Into<String>, payload: T) -> Self {
        Self {
            id,
            protocol_version: PROTOCOL_VERSION.to_string(),
            kind: kind.into(),
            payload,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SidecarEvent {
    pub(crate) id: String,
    pub(crate) event: String,
    #[serde(default)]
    pub(crate) status: String,
    #[serde(default)]
    pub(crate) phase: String,
    #[serde(default)]
    pub(crate) progress_percent: u32,
    #[serde(default)]
    pub(crate) current_page: Option<u32>,
    #[serde(default)]
    pub(crate) retryable: bool,
    #[serde(default)]
    pub(crate) error_code: String,
    #[serde(default)]
    pub(crate) message: String,
    #[serde(default)]
    pub(crate) sidecar_version: String,
    #[serde(default)]
    pub(crate) pdf2zh_version: String,
    #[serde(default)]
    pub(crate) babeldoc_version: String,
    #[serde(default)]
    pub(crate) python: String,
    #[serde(default)]
    pub(crate) mono_pdf_path: String,
    #[serde(default)]
    pub(crate) dual_pdf_path: String,
    #[serde(default)]
    pub(crate) artifact_scope: String,
    #[serde(default)]
    pub(crate) artifact_pages: String,
}

impl SidecarEvent {
    pub(crate) fn terminal_success(&self) -> bool {
        matches!(
            self.event.as_str(),
            "finish" | "probe_result" | "preflight_result"
        )
    }

    pub(crate) fn terminal_error(&self) -> bool {
        self.event == "error"
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProbePayload {
    pub(crate) app_data_dir: String,
    pub(crate) log_dir: String,
    pub(crate) cache_dir: String,
    pub(crate) artifacts_dir: String,
    pub(crate) tmp_dir: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TranslationEnginePayload {
    pub(crate) provider: String,
    pub(crate) base_url: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) api_key: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TranslatePayload {
    pub(crate) job_id: String,
    pub(crate) input_pdf_path: String,
    pub(crate) output_dir: String,
    pub(crate) log_dir: String,
    pub(crate) cache_dir: String,
    pub(crate) source_lang: String,
    pub(crate) target_lang: String,
    pub(crate) pages: Option<String>,
    pub(crate) artifact_mode: String,
    pub(crate) only_include_translated_page: bool,
    pub(crate) force_refresh: bool,
    pub(crate) engine: TranslationEnginePayload,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PreflightPayload {
    pub(crate) log_dir: String,
    pub(crate) cache_dir: String,
    pub(crate) output_dir: String,
    pub(crate) source_lang: String,
    pub(crate) target_lang: String,
    pub(crate) engine: TranslationEnginePayload,
}
