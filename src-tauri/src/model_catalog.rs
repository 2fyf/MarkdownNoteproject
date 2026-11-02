use serde::{Deserialize, Serialize};
use std::{env, fs, path::PathBuf, sync::OnceLock};

const MODELS_DEV_SNAPSHOT: &str = include_str!("../models.dev.api.json");
const MODELS_DEV_JSON_ENV: &str = "MARKDOWNNOTE_MODELS_DEV_JSON";
const DEFAULT_CONTEXT_TOKENS: usize = 32_768;
const DEFAULT_OUTPUT_TOKENS: usize = 8_192;
const MAX_EVIDENCE_TOKENS: usize = 256_000;
const MAX_JUDGE_EVIDENCE_TOKENS: usize = 64_000;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ResolvedModelProfile {
    pub model_id: String,
    pub context_tokens: usize,
    pub output_tokens: usize,
    pub source: String,
    pub matched_provider: Option<String>,
    pub matched_model: Option<String>,
    pub tool_call: Option<bool>,
    pub input_modalities: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ModelContextBudget {
    pub model_context_tokens: usize,
    pub model_output_tokens: usize,
    pub evidence_tokens: usize,
    pub judge_evidence_tokens: usize,
    pub max_context_chars: usize,
    pub max_quote_chars: usize,
    pub max_initial_citations: usize,
    pub max_accumulated_citations: usize,
    pub judge_max_citations: usize,
    pub judge_quote_chars: usize,
    pub source: String,
}

impl Default for ModelContextBudget {
    fn default() -> Self {
        Self::from_model_limits(DEFAULT_CONTEXT_TOKENS, DEFAULT_OUTPUT_TOKENS, "default")
    }
}

impl ModelContextBudget {
    pub(crate) fn from_model_limits(
        context_tokens: usize,
        output_tokens: usize,
        source: &str,
    ) -> Self {
        let context_tokens = context_tokens.max(8_192);
        let output_tokens = output_tokens.max(1_024);
        let reserved_tokens = output_tokens
            .saturating_add(context_tokens / 5)
            .saturating_add(4_096)
            .min(context_tokens.saturating_sub(4_096));
        let available_tokens = context_tokens.saturating_sub(reserved_tokens).max(8_192);
        let evidence_tokens = available_tokens.clamp(8_192, MAX_EVIDENCE_TOKENS);
        let judge_evidence_tokens = (context_tokens / 4).clamp(8_192, MAX_JUDGE_EVIDENCE_TOKENS);
        let max_accumulated_citations = (evidence_tokens / 1_200).clamp(24, 256);
        let max_initial_citations = (max_accumulated_citations / 2).clamp(12, 128);
        let max_context_chars = evidence_tokens.saturating_mul(3);
        let max_quote_chars = (max_context_chars / max_accumulated_citations).clamp(1_800, 8_000);
        let judge_max_citations = (judge_evidence_tokens / 1_000).clamp(24, 80);
        let judge_quote_chars =
            (judge_evidence_tokens.saturating_mul(3) / judge_max_citations).clamp(900, 3_000);
        Self {
            model_context_tokens: context_tokens,
            model_output_tokens: output_tokens,
            evidence_tokens,
            judge_evidence_tokens,
            max_context_chars,
            max_quote_chars,
            max_initial_citations,
            max_accumulated_citations,
            judge_max_citations,
            judge_quote_chars,
            source: source.to_string(),
        }
    }
}

pub(crate) fn resolve_model_profile(
    provider_type: &str,
    base_url: &str,
    model_id: &str,
) -> ResolvedModelProfile {
    let requested_model = model_id.trim();
    let Some(catalog) = model_catalog() else {
        return fallback_profile(requested_model, "catalog_unavailable");
    };

    if let Some((provider_id, model)) =
        find_model(catalog, provider_type, base_url, requested_model)
    {
        let context_tokens = model
            .limit
            .as_ref()
            .and_then(|limit| limit.context)
            .unwrap_or(DEFAULT_CONTEXT_TOKENS);
        let output_tokens = model
            .limit
            .as_ref()
            .and_then(|limit| limit.output)
            .unwrap_or(DEFAULT_OUTPUT_TOKENS);
        return ResolvedModelProfile {
            model_id: requested_model.to_string(),
            context_tokens,
            output_tokens,
            source: "models.dev".to_string(),
            matched_provider: Some(provider_id.to_string()),
            matched_model: Some(model.id.clone()),
            tool_call: model.tool_call,
            input_modalities: model
                .modalities
                .as_ref()
                .map(|modalities| modalities.input.clone())
                .unwrap_or_default(),
        };
    }

    fallback_profile(requested_model, "unknown_model")
}

pub(crate) fn warmup_model_catalog() {
    let _ = model_catalog();
}

impl ResolvedModelProfile {
    pub(crate) fn context_budget(&self) -> ModelContextBudget {
        ModelContextBudget::from_model_limits(self.context_tokens, self.output_tokens, &self.source)
    }
}

fn fallback_profile(model_id: &str, source: &str) -> ResolvedModelProfile {
    ResolvedModelProfile {
        model_id: model_id.to_string(),
        context_tokens: DEFAULT_CONTEXT_TOKENS,
        output_tokens: DEFAULT_OUTPUT_TOKENS,
        source: source.to_string(),
        matched_provider: None,
        matched_model: None,
        tool_call: None,
        input_modalities: Vec::new(),
    }
}

fn model_catalog() -> Option<&'static serde_json::Map<String, serde_json::Value>> {
    static CATALOG: OnceLock<Option<serde_json::Map<String, serde_json::Value>>> = OnceLock::new();
    CATALOG.get_or_init(load_model_catalog).as_ref()
}

fn load_model_catalog() -> Option<serde_json::Map<String, serde_json::Value>> {
    for path in runtime_catalog_paths() {
        match fs::read_to_string(&path) {
            Ok(content) => match parse_catalog_json(&content) {
                Some(catalog) => return Some(catalog),
                None => log::warn!("Ignoring invalid models.dev catalog at {}", path.display()),
            },
            Err(err) => {
                if env::var_os(MODELS_DEV_JSON_ENV).is_some() || path.exists() {
                    log::warn!(
                        "Could not read models.dev catalog at {}: {err}",
                        path.display()
                    );
                }
            }
        }
    }
    parse_catalog_json(MODELS_DEV_SNAPSHOT)
}

fn runtime_catalog_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(path) = env::var_os(MODELS_DEV_JSON_ENV) {
        paths.push(PathBuf::from(path));
    }
    paths.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("models.dev.api.json"));
    paths
}

fn parse_catalog_json(content: &str) -> Option<serde_json::Map<String, serde_json::Value>> {
    serde_json::from_str::<serde_json::Value>(content)
        .ok()
        .and_then(|value| value.as_object().cloned())
}

fn find_model<'a>(
    catalog: &'a serde_json::Map<String, serde_json::Value>,
    provider_type: &str,
    base_url: &str,
    model_id: &str,
) -> Option<(&'a str, CatalogModel)> {
    let normalized_model = normalize_model_id(model_id);
    let provider_hint = provider_hint(provider_type, base_url);
    let mut fallback = None;

    for (provider_id, provider_value) in catalog {
        let Ok(provider) = serde_json::from_value::<CatalogProvider>(provider_value.clone()) else {
            continue;
        };
        let provider_matches = provider_hint
            .as_deref()
            .is_some_and(|hint| provider_id == hint || provider.id.as_deref() == Some(hint));
        for (candidate_id, model_value) in provider.models {
            let Ok(model) = serde_json::from_value::<CatalogModel>(model_value) else {
                continue;
            };
            let candidate_matches = normalize_model_id(&candidate_id) == normalized_model
                || normalize_model_id(&model.id) == normalized_model;
            if !candidate_matches {
                continue;
            }
            if provider_matches {
                return Some((provider_id.as_str(), model));
            }
            if fallback.is_none() {
                fallback = Some((provider_id.as_str(), model));
            }
        }
    }

    fallback
}

fn provider_hint(provider_type: &str, base_url: &str) -> Option<String> {
    let provider_type = provider_type.trim().to_lowercase();
    if matches!(provider_type.as_str(), "openai" | "deepseek" | "openrouter") {
        return Some(provider_type);
    }
    let base_url = base_url.to_lowercase();
    for hint in [
        ("api.openai.com", "openai"),
        ("api.deepseek.com", "deepseek"),
        ("openrouter.ai", "openrouter"),
        ("dashscope", "alibaba"),
        ("siliconflow", "siliconflow"),
        ("moonshot", "moonshotai"),
        ("volces", "volces"),
        ("anthropic", "anthropic"),
        ("googleapis", "google"),
    ] {
        if base_url.contains(hint.0) {
            return Some(hint.1.to_string());
        }
    }
    None
}

fn normalize_model_id(value: &str) -> String {
    value
        .trim()
        .trim_start_matches("models/")
        .to_lowercase()
        .replace('_', "-")
}

#[derive(Deserialize)]
struct CatalogProvider {
    id: Option<String>,
    #[serde(default)]
    models: serde_json::Map<String, serde_json::Value>,
}

#[derive(Deserialize)]
struct CatalogModel {
    id: String,
    #[serde(default)]
    tool_call: Option<bool>,
    #[serde(default)]
    modalities: Option<CatalogModalities>,
    #[serde(default)]
    limit: Option<CatalogLimit>,
}

#[derive(Deserialize)]
struct CatalogModalities {
    #[serde(default)]
    input: Vec<String>,
}

#[derive(Deserialize)]
struct CatalogLimit {
    context: Option<usize>,
    output: Option<usize>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_known_model_context_from_snapshot() {
        let profile = resolve_model_profile("openai", "https://api.openai.com/v1", "gpt-4o");

        assert_eq!(profile.source, "models.dev");
        assert!(profile.context_tokens >= 100_000);
        assert_eq!(profile.matched_provider.as_deref(), Some("openai"));
    }

    #[test]
    fn budget_scales_with_long_context_model() {
        let budget = ModelContextBudget::from_model_limits(256_000, 16_384, "test");

        assert!(budget.max_context_chars > 300_000);
        assert!(budget.max_accumulated_citations > 100);
        assert!(budget.judge_max_citations >= 60);
    }

    #[test]
    fn unknown_model_uses_conservative_default() {
        let profile = resolve_model_profile(
            "openai-compatible",
            "https://example.test/v1",
            "not-a-known-model",
        );

        assert_eq!(profile.source, "unknown_model");
        assert_eq!(profile.context_tokens, DEFAULT_CONTEXT_TOKENS);
    }
}
