use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension};

use crate::{
    model_catalog, stable_text_hash, AppDatabase, ModelProviderModelInput,
    ModelProviderModelOutput, ModelProviderOutput, OpenAiCompatibleProvider, StoredModelProvider,
    StoredProviderModel,
};

pub(crate) fn new_model_provider_id(name: &str) -> String {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    stable_text_hash(&format!("provider:{}:{nonce}", name.trim()))
}

pub(crate) fn load_model_provider_outputs(
    conn: &Connection,
) -> Result<Vec<ModelProviderOutput>, String> {
    let providers = load_model_providers(conn)?;
    Ok(providers.into_iter().map(provider_output).collect())
}

pub(crate) fn load_default_model_provider(
    conn: &Connection,
) -> Result<Option<StoredModelProvider>, String> {
    conn.query_row(
        "SELECT id, name, provider_type, base_url, model, models_json, default_model_key, enabled, is_default, has_api_key, api_key_local
         FROM model_providers
         ORDER BY is_default DESC, updated_at DESC
         LIMIT 1",
        [],
        read_stored_provider_row,
    )
    .optional()
    .map_err(|err| format!("Failed to load default model provider: {err}"))
}

pub(crate) fn load_model_provider_by_id(
    conn: &Connection,
    provider_id: &str,
) -> Result<Option<StoredModelProvider>, String> {
    conn.query_row(
        "SELECT id, name, provider_type, base_url, model, models_json, default_model_key, enabled, is_default, has_api_key, api_key_local
         FROM model_providers
         WHERE id = ?1
         LIMIT 1",
        params![provider_id],
        read_stored_provider_row,
    )
    .optional()
    .map_err(|err| format!("Failed to load model provider: {err}"))
}

pub(crate) fn provider_output(provider: StoredModelProvider) -> ModelProviderOutput {
    ModelProviderOutput {
        id: provider.id,
        name: provider.name,
        provider_type: provider.provider_type,
        base_url: provider.base_url,
        models: provider
            .models
            .into_iter()
            .map(|model| ModelProviderModelOutput {
                key: model.key,
                model_id: model.model_id,
                nickname: model.nickname,
                capabilities: model.capabilities,
                enabled: model.enabled,
                is_default_chat_model: model.is_default_chat_model,
                context_window_override: model.context_window_override,
                detected_context_window: model.detected_context_window,
            })
            .collect(),
        default_model_key: provider.default_model_key,
        enabled: provider.enabled,
        is_default: provider.is_default,
        has_api_key: provider.has_api_key,
    }
}

pub(crate) fn normalize_provider_models(
    provider_type: &str,
    provider_id: &str,
    inputs: Vec<ModelProviderModelInput>,
) -> Result<Vec<StoredProviderModel>, String> {
    let mut models = Vec::new();
    for input in inputs {
        let model_id = input.model_id.trim();
        if model_id.is_empty() {
            continue;
        }
        let key = input
            .key
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
            .unwrap_or_else(|| stable_text_hash(&format!("{provider_id}:{model_id}")));
        let nickname = input
            .nickname
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(model_id)
            .to_string();
        let capabilities = normalize_model_capabilities(
            input
                .capabilities
                .unwrap_or_else(|| infer_model_capabilities(provider_type, model_id)),
        );
        models.push(StoredProviderModel {
            key,
            model_id: model_id.to_string(),
            nickname,
            capabilities,
            enabled: input.enabled.unwrap_or(true),
            is_default_chat_model: input.is_default_chat_model.unwrap_or(false),
            // 0 (or absent) means "auto" — only a positive value is a real override.
            context_window_override: input.context_window_override.filter(|value| *value >= 1024),
            detected_context_window: input.detected_context_window.filter(|value| *value >= 1024),
        });
    }
    if models.is_empty() {
        return Err("At least one enabled or configured chat model is required".to_string());
    }
    if !models.iter().any(|model| model.enabled) {
        return Err("At least one chat model must be enabled".to_string());
    }
    let default_key = resolve_default_model_key(&models, None)?;
    apply_default_model_key(&mut models, &default_key);
    Ok(models)
}

pub(crate) fn apply_default_model_key(models: &mut [StoredProviderModel], default_model_key: &str) {
    for model in models {
        model.is_default_chat_model = model.key == default_model_key;
    }
}

pub(crate) fn resolve_default_model_key(
    models: &[StoredProviderModel],
    requested_default_key: Option<&str>,
) -> Result<String, String> {
    if let Some(requested_default_key) = requested_default_key
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if let Some(model) = models
            .iter()
            .find(|model| model.key == requested_default_key && model.enabled)
        {
            return Ok(model.key.clone());
        }
    }
    if let Some(model) = models
        .iter()
        .find(|model| model.enabled && model.is_default_chat_model)
    {
        return Ok(model.key.clone());
    }
    models
        .iter()
        .find(|model| model.enabled)
        .map(|model| model.key.clone())
        .ok_or_else(|| "At least one chat model must be enabled".to_string())
}

pub(crate) fn is_openai_compatible_provider_type(provider_type: &str) -> bool {
    matches!(
        provider_type.trim().to_lowercase().as_str(),
        "openai-compatible" | "openai" | "deepseek" | "openrouter"
    )
}

pub(crate) fn legacy_api_key_secret_ref(provider_id: &str) -> String {
    format!("model-provider:{provider_id}:api-key")
}

pub(crate) fn read_local_model_provider_api_key(
    database: &AppDatabase,
    provider_id: &str,
) -> Result<Option<String>, String> {
    let provider_id = provider_id.trim();
    if provider_id.is_empty() {
        return Ok(None);
    }
    let conn = database
        .conn
        .lock()
        .map_err(|_| "SQLite lock was poisoned".to_string())?;
    conn.query_row(
        "SELECT api_key_local FROM model_providers WHERE id = ?1 LIMIT 1",
        params![provider_id],
        |row| row.get::<_, String>(0),
    )
    .optional()
    .map_err(|err| format!("Failed to read local provider API key: {err}"))
    .map(|value| {
        value
            .map(|key| key.trim().to_string())
            .filter(|key| !key.is_empty())
    })
}

pub(crate) fn resolve_chat_provider(
    database: &AppDatabase,
    selected_provider_id: &str,
    model_key: Option<&str>,
) -> Result<(OpenAiCompatibleProvider, String), String> {
    let conn = database
        .conn
        .lock()
        .map_err(|_| "SQLite lock was poisoned".to_string())?;
    let stored_provider = if selected_provider_id.is_empty() {
        load_default_model_provider(&conn)?
    } else {
        load_model_provider_by_id(&conn, selected_provider_id)?
    };
    let Some(stored_provider) = stored_provider else {
        return Err(if selected_provider_id.is_empty() {
            "No model provider is configured. Open Settings and add a chat model provider."
                .to_string()
        } else {
            "The selected model provider is unavailable. Re-open the model selector or update Settings."
                .to_string()
        });
    };
    let selected_model = pick_chat_model(&stored_provider, model_key)?;
    let api_key =
        Some(stored_provider.api_key_local.trim().to_string()).filter(|value| !value.is_empty());
    if api_key.is_none() {
        return Err(
            "No local API key is saved for this provider. Re-save the API key in Model settings."
                .to_string(),
        );
    }
    let mut model_profile = model_catalog::resolve_model_profile(
        &stored_provider.provider_type,
        &stored_provider.base_url,
        &selected_model.model_id,
    );
    // Use the model's REAL context window when we know it: a user override always
    // wins, then the window auto-detected from the provider's /models endpoint,
    // then the bundled catalog / default the profile already carries.
    apply_context_window_override(
        &mut model_profile,
        selected_model.context_window_override,
        selected_model.detected_context_window,
    );
    let context_budget = model_profile.context_budget();
    let provider = OpenAiCompatibleProvider {
        base_url: stored_provider.base_url.clone(),
        api_key,
        model: selected_model.model_id.clone(),
        capabilities: selected_model.capabilities.clone(),
        model_profile,
        context_budget,
    };
    let provider_label = format!("{} · {}", stored_provider.name, selected_model.nickname);
    Ok((provider, provider_label))
}

/// Apply the user override / auto-detected context window onto a resolved model
/// profile. Precedence: user override > detected (`/models`) > the profile's own
/// value (bundled catalog or default). Only positive windows (>= 1024 tokens) are
/// honored, so an empty/"auto" field leaves the catalog value untouched.
pub(crate) fn apply_context_window_override(
    profile: &mut model_catalog::ResolvedModelProfile,
    override_window: Option<u32>,
    detected_window: Option<u32>,
) {
    if let Some(window) = override_window.filter(|value| *value >= 1024) {
        profile.context_tokens = window as usize;
        profile.source = "user-override".to_string();
    } else if let Some(window) = detected_window.filter(|value| *value >= 1024) {
        profile.context_tokens = window as usize;
        profile.source = "models-endpoint".to_string();
    }
}

pub(crate) fn pick_chat_model<'a>(
    provider: &'a StoredModelProvider,
    model_key: Option<&str>,
) -> Result<&'a StoredProviderModel, String> {
    let requested_key = model_key.map(str::trim).filter(|value| !value.is_empty());
    if let Some(requested_key) = requested_key {
        if let Some(model) = provider
            .models
            .iter()
            .find(|model| model.key == requested_key && model.enabled)
        {
            return Ok(model);
        }
    }
    provider
        .models
        .iter()
        .find(|model| model.enabled && model.key == provider.default_model_key)
        .or_else(|| provider.models.iter().find(|model| model.enabled))
        .ok_or_else(|| "The selected model provider has no enabled chat models.".to_string())
}

fn load_model_providers(conn: &Connection) -> Result<Vec<StoredModelProvider>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, name, provider_type, base_url, model, models_json, default_model_key, enabled, is_default, has_api_key, api_key_local
             FROM model_providers
             ORDER BY is_default DESC, lower(name)",
        )
        .map_err(|err| format!("Failed to prepare model providers: {err}"))?;

    let rows = stmt
        .query_map([], read_stored_provider_row)
        .map_err(|err| format!("Failed to load model providers: {err}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| format!("Failed to read model providers: {err}"))?;
    Ok(rows)
}

fn read_stored_provider_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredModelProvider> {
    let provider_id: String = row.get(0)?;
    let provider_type: String = row.get(2)?;
    let legacy_model: String = row.get(4)?;
    let models_json: String = row.get(5)?;
    let default_model_key: String = row.get(6)?;
    let models = decode_stored_provider_models(
        &provider_id,
        &provider_type,
        &legacy_model,
        &models_json,
        &default_model_key,
    )
    .map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, err)),
        )
    })?;
    let resolved_default_model_key = models
        .iter()
        .find(|model| model.enabled && model.is_default_chat_model)
        .or_else(|| models.iter().find(|model| model.enabled))
        .map(|model| model.key.clone())
        .unwrap_or(default_model_key);
    Ok(StoredModelProvider {
        id: provider_id,
        name: row.get(1)?,
        provider_type,
        base_url: row.get(3)?,
        models,
        default_model_key: resolved_default_model_key,
        enabled: row.get(7)?,
        is_default: row.get(8)?,
        has_api_key: row.get(9)?,
        api_key_local: row.get(10)?,
    })
}

fn decode_stored_provider_models(
    provider_id: &str,
    provider_type: &str,
    legacy_model: &str,
    models_json: &str,
    default_model_key: &str,
) -> Result<Vec<StoredProviderModel>, String> {
    let mut models =
        serde_json::from_str::<Vec<StoredProviderModel>>(models_json).unwrap_or_default();
    if models.is_empty() && !legacy_model.trim().is_empty() {
        models.push(default_provider_model(
            provider_id,
            provider_type,
            legacy_model.trim(),
        ));
    }
    if models.is_empty() {
        return Ok(Vec::new());
    }
    let resolved_default = resolve_default_model_key(&models, Some(default_model_key))?;
    apply_default_model_key(&mut models, &resolved_default);
    Ok(models)
}

fn default_provider_model(
    provider_id: &str,
    provider_type: &str,
    model_id: &str,
) -> StoredProviderModel {
    StoredProviderModel {
        key: stable_text_hash(&format!("{provider_id}:{model_id}")),
        model_id: model_id.to_string(),
        nickname: model_id.to_string(),
        capabilities: infer_model_capabilities(provider_type, model_id),
        enabled: true,
        is_default_chat_model: true,
        context_window_override: None,
        detected_context_window: None,
    }
}

fn infer_model_capabilities(provider_type: &str, model_id: &str) -> Vec<String> {
    let normalized = model_id.to_lowercase();
    let mut caps = vec!["text".to_string()];
    let has_vision = normalized.contains("gpt-4o")
        || normalized.contains("gpt-4.1")
        || normalized.contains("gpt-5")
        || normalized.contains("o3")
        || normalized.contains("o4")
        || normalized.contains("claude-3")
        || normalized.contains("claude-4")
        || normalized.contains("gemini")
        || normalized.contains("vision")
        || normalized.contains("vl")
        || normalized.contains("pixtral");
    let has_reasoning = normalized.contains("reason")
        || normalized.contains("reasoner")
        || normalized.contains("thinking")
        || normalized.contains("o3")
        || normalized.contains("o4")
        || normalized.contains("gpt-5");
    let has_tool_use = matches!(
        provider_type.trim(),
        "openai" | "openrouter" | "openai-compatible"
    ) || normalized.contains("tool")
        || normalized.contains("deepseek");

    if has_vision {
        caps.push("vision".to_string());
    }
    if has_reasoning {
        caps.push("reasoning".to_string());
    }
    if has_tool_use {
        caps.push("tool_use".to_string());
    }
    normalize_model_capabilities(caps)
}

fn normalize_model_capabilities(capabilities: Vec<String>) -> Vec<String> {
    let mut normalized = Vec::new();
    for capability in capabilities {
        let capability = capability.trim().to_lowercase();
        if !matches!(
            capability.as_str(),
            "text" | "vision" | "reasoning" | "tool_use"
        ) {
            continue;
        }
        if !normalized.contains(&capability) {
            normalized.push(capability);
        }
    }
    if !normalized.iter().any(|capability| capability == "text") {
        normalized.insert(0, "text".to_string());
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(context_tokens: usize) -> model_catalog::ResolvedModelProfile {
        model_catalog::ResolvedModelProfile {
            model_id: "m".into(),
            context_tokens,
            output_tokens: 4096,
            source: "models.dev".into(),
            matched_provider: None,
            matched_model: None,
            tool_call: Some(true),
            input_modalities: Vec::new(),
        }
    }

    #[test]
    fn context_window_override_precedence() {
        // No override / no detection -> catalog value untouched.
        let mut none = profile(32_000);
        apply_context_window_override(&mut none, None, None);
        assert_eq!(none.context_tokens, 32_000);

        // Detected (/models) beats the bundled catalog.
        let mut detected = profile(32_000);
        apply_context_window_override(&mut detected, None, Some(128_000));
        assert_eq!(detected.context_tokens, 128_000);
        assert_eq!(detected.source, "models-endpoint");

        // A user override beats both detection and catalog.
        let mut overridden = profile(32_000);
        apply_context_window_override(&mut overridden, Some(200_000), Some(128_000));
        assert_eq!(overridden.context_tokens, 200_000);
        assert_eq!(overridden.source, "user-override");

        // Sub-1024 values are treated as "auto" and ignored.
        let mut tiny = profile(32_000);
        apply_context_window_override(&mut tiny, Some(0), Some(500));
        assert_eq!(tiny.context_tokens, 32_000);
    }
}
