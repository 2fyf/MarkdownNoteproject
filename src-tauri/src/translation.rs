use std::time::Duration;

use crate::{
    llm, normalize_base_url, truncate_for_error, MicrosoftTranslateResponseItem,
    MicrosoftTranslatorProvider, OpenAiChatRequest, OpenAiChatResponse, OpenAiCompatibleProvider,
    TranslationProvider,
};

impl TranslationProvider {
    pub(crate) fn cache_key(&self) -> String {
        match self {
            TranslationProvider::LocalPlaceholder => "local-placeholder".to_string(),
            TranslationProvider::GoogleWeb => "google-web".to_string(),
            TranslationProvider::Microsoft(provider) => {
                format!("microsoft:{}", normalize_base_url(&provider.endpoint))
            }
            TranslationProvider::OpenAiCompatible(provider) => {
                format!(
                    "openai-compatible:{}:{}",
                    normalize_base_url(&provider.base_url),
                    provider.model
                )
            }
            TranslationProvider::Unavailable { cache_key, .. } => cache_key.clone(),
        }
    }

    pub(crate) fn label(&self) -> String {
        match self {
            TranslationProvider::LocalPlaceholder => "local-placeholder".to_string(),
            TranslationProvider::GoogleWeb => "google-web".to_string(),
            TranslationProvider::Microsoft(_) => "microsoft".to_string(),
            TranslationProvider::OpenAiCompatible(provider) => {
                format!("openai-compatible/{}", provider.model)
            }
            TranslationProvider::Unavailable { label, .. } => label.clone(),
        }
    }
}

pub(crate) async fn translate_with_provider(
    source_text: &str,
    target_lang: &str,
    provider: &TranslationProvider,
) -> Result<String, String> {
    match provider {
        TranslationProvider::LocalPlaceholder => {
            Ok(local_placeholder_translation(source_text, target_lang))
        }
        TranslationProvider::GoogleWeb => translate_with_google_web(source_text, target_lang).await,
        TranslationProvider::Microsoft(provider) => {
            translate_with_microsoft(source_text, target_lang, provider).await
        }
        TranslationProvider::OpenAiCompatible(provider) => {
            translate_with_openai_compatible(source_text, target_lang, provider).await
        }
        TranslationProvider::Unavailable { message, .. } => Err(message.clone()),
    }
}

fn local_placeholder_translation(source_text: &str, target_lang: &str) -> String {
    if target_lang.eq_ignore_ascii_case("zh") {
        format!("【本地占位翻译】尚未配置翻译 Provider。\n\n{source_text}")
    } else {
        format!(
            "[Local translation placeholder] No translation provider is configured yet.\n\n{source_text}"
        )
    }
}

async fn translate_with_google_web(source_text: &str, target_lang: &str) -> Result<String, String> {
    if source_text.chars().count() > 5000 {
        return Err(
            "Google Web translation supports up to 5000 characters. Select a shorter passage or use the LLM provider.".to_string()
        );
    }

    let client = crate::net::client_builder()
        .timeout(Duration::from_secs(30))
        .user_agent(
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
             (KHTML, like Gecko) Chrome/124.0 Safari/537.36",
        )
        .build()
        .map_err(|err| format!("Failed to create Google web translation client: {err}"))?;
    let target = google_web_language_code(target_lang);
    let response = client
        .get("https://translate.google.com/m")
        .query(&[("sl", "auto"), ("tl", target.as_str()), ("q", source_text)])
        .send()
        .await
        .map_err(|err| format!("Google web translation request failed: {err}"))?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(format!(
            "Google web translation returned {status}: {}",
            truncate_for_error(&body, 600)
        ));
    }

    let body = response
        .text()
        .await
        .map_err(|err| format!("Failed to read Google web translation response: {err}"))?;
    extract_google_web_translation(&body)
        .map(|text| decode_html_entities(text.trim()))
        .filter(|text| !text.is_empty())
        .ok_or_else(|| {
            "Google web translation response did not contain translated text".to_string()
        })
}

fn extract_google_web_translation(body: &str) -> Option<&str> {
    for marker in ["class=\"result-container\"", "class=\"t0\""] {
        let Some(marker_index) = body.find(marker) else {
            continue;
        };
        let after_marker = &body[marker_index..];
        let Some(start) = after_marker.find('>').map(|index| index + 1) else {
            continue;
        };
        let after_start = &after_marker[start..];
        let Some(end) = after_start.find('<') else {
            continue;
        };
        let result = after_start[..end].trim();
        if !result.is_empty() {
            return Some(result);
        }
    }
    None
}

fn google_web_language_code(lang: &str) -> String {
    match lang.to_lowercase().as_str() {
        "zh" | "zh-cn" | "cn" => "zh-CN".to_string(),
        "zh-tw" => "zh-TW".to_string(),
        value => value.to_string(),
    }
}

async fn translate_with_microsoft(
    source_text: &str,
    target_lang: &str,
    provider: &MicrosoftTranslatorProvider,
) -> Result<String, String> {
    if source_text.chars().count() > 5000 {
        return Err(
            "Microsoft Translator supports up to 5000 characters per request. Select a shorter passage."
                .to_string(),
        );
    }

    let client = crate::net::client_builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|err| format!("Failed to create Microsoft Translator client: {err}"))?;
    let endpoint = microsoft_translate_endpoint(&provider.endpoint);
    let target = microsoft_language_code(target_lang);
    let response = client
        .post(endpoint)
        .query(&[("api-version", "3.0"), ("to", target.as_str())])
        .header("Ocp-Apim-Subscription-Key", &provider.api_key)
        .header("Ocp-Apim-Subscription-Region", &provider.region)
        .json(&serde_json::json!([{ "Text": source_text }]))
        .send()
        .await
        .map_err(|err| format!("Microsoft Translator request failed: {err}"))?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(format!(
            "Microsoft Translator returned {status}: {}",
            truncate_for_error(&body, 600)
        ));
    }

    let response = response
        .json::<Vec<MicrosoftTranslateResponseItem>>()
        .await
        .map_err(|err| format!("Failed to decode Microsoft Translator response: {err}"))?;
    response
        .into_iter()
        .next()
        .and_then(|item| item.translations.into_iter().next())
        .map(|translation| translation.text.trim().to_string())
        .filter(|text| !text.is_empty())
        .ok_or_else(|| "Microsoft Translator returned an empty response".to_string())
}

fn microsoft_translate_endpoint(endpoint: &str) -> String {
    let endpoint = normalize_base_url(endpoint);
    if endpoint.ends_with("/translate") {
        endpoint
    } else {
        format!("{endpoint}/translate")
    }
}

fn microsoft_language_code(lang: &str) -> String {
    match lang.to_lowercase().as_str() {
        "zh" | "zh-cn" | "cn" => "zh-Hans".to_string(),
        "zh-tw" => "zh-Hant".to_string(),
        value => value.to_string(),
    }
}

fn decode_html_entities(value: &str) -> String {
    let mut result = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(entity_start) = rest.find('&') {
        result.push_str(&rest[..entity_start]);
        rest = &rest[entity_start..];
        let Some(entity_end) = rest.find(';') else {
            result.push_str(rest);
            return result;
        };
        let entity = &rest[1..entity_end];
        if let Some(decoded) = decode_html_entity(entity) {
            result.push(decoded);
        } else {
            result.push_str(&rest[..=entity_end]);
        }
        rest = &rest[entity_end + 1..];
    }
    result.push_str(rest);
    result
}

fn decode_html_entity(entity: &str) -> Option<char> {
    match entity {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" | "#39" => Some('\''),
        _ if entity.starts_with("#x") || entity.starts_with("#X") => {
            u32::from_str_radix(&entity[2..], 16)
                .ok()
                .and_then(char::from_u32)
        }
        _ if entity.starts_with('#') => entity[1..].parse::<u32>().ok().and_then(char::from_u32),
        _ => None,
    }
}

async fn translate_with_openai_compatible(
    source_text: &str,
    target_lang: &str,
    provider: &OpenAiCompatibleProvider,
) -> Result<String, String> {
    let client = crate::net::client_builder()
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(|err| format!("Failed to create translation client: {err}"))?;
    let endpoint = format!(
        "{}/chat/completions",
        normalize_base_url(&provider.base_url)
    );
    let target_language = translation_language_name(target_lang);
    let request = OpenAiChatRequest {
        model: provider.model.clone(),
        temperature: 0.2,
        stream: None,
        messages: vec![
            llm::chat::text_message(
                "system",
                format!(
                    "You are a precise academic translator. Translate the user's text into {target_language}. Preserve equations, citations, Markdown, names, and technical terms. Return only the translated text."
                ),
            ),
            llm::chat::text_message("user", source_text.to_string()),
        ],
    };
    let mut builder = client.post(endpoint).json(&request);
    if let Some(api_key) = &provider.api_key {
        builder = builder.bearer_auth(api_key);
    }

    let response = builder
        .send()
        .await
        .map_err(|err| format!("Translation provider request failed: {err}"))?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(format!(
            "Translation provider returned {status}: {}",
            truncate_for_error(&body, 600)
        ));
    }

    let response = response
        .json::<OpenAiChatResponse>()
        .await
        .map_err(|err| format!("Failed to decode translation response: {err}"))?;
    response
        .choices
        .into_iter()
        .next()
        .map(|choice| llm::chat::extract_chat_response_text(&choice.message.content))
        .filter(|text| !text.is_empty())
        .ok_or_else(|| "Translation provider returned an empty response".to_string())
}

fn translation_language_name(lang: &str) -> String {
    match lang.to_lowercase().as_str() {
        "zh" | "zh-cn" | "cn" => "Simplified Chinese".to_string(),
        "zh-tw" => "Traditional Chinese".to_string(),
        "en" => "English".to_string(),
        "ja" | "jp" => "Japanese".to_string(),
        "ko" => "Korean".to_string(),
        "fr" => "French".to_string(),
        "de" => "German".to_string(),
        "es" => "Spanish".to_string(),
        value => value.to_string(),
    }
}
