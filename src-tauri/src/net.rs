//! Shared outbound HTTP configuration.
//!
//! GUI apps launched from Finder/Dock don't inherit the shell's `HTTPS_PROXY`
//! env vars, and reqwest doesn't read the macOS system proxy — so users behind
//! a proxy/VPN (e.g. to reach huggingface.co) had every outbound request fail
//! with "error sending request". This module holds a process-global proxy URL
//! (seeded from `app_settings` at startup, updated from Settings) and a single
//! `client_builder()` every reqwest client goes through so the proxy applies
//! uniformly (trending, web search, translation, model APIs, update check…).

use std::sync::RwLock;

static PROXY_URL: RwLock<Option<String>> = RwLock::new(None);

/// Normalize a user-entered proxy value: trim, treat empty as "no proxy", and
/// default a scheme-less `host:port` to `http://` (what Clash/V2ray expose).
fn normalize(url: Option<String>) -> Option<String> {
    let trimmed = url.map(|value| value.trim().to_string())?;
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.contains("://") {
        Some(trimmed)
    } else {
        Some(format!("http://{trimmed}"))
    }
}

/// Set (or clear) the global proxy. Called at startup and whenever the user
/// saves the setting.
pub(crate) fn set_proxy(url: Option<String>) {
    let normalized = normalize(url);
    if let Ok(mut guard) = PROXY_URL.write() {
        *guard = normalized;
    }
}

/// The currently-configured proxy URL, if any.
pub(crate) fn proxy_url() -> Option<String> {
    PROXY_URL.read().ok().and_then(|guard| guard.clone())
}

/// Apply the configured proxy (if any) to a reqwest client builder. An invalid
/// URL is logged and skipped rather than breaking client construction.
pub(crate) fn apply_proxy(builder: reqwest::ClientBuilder) -> reqwest::ClientBuilder {
    match proxy_url() {
        Some(url) => match reqwest::Proxy::all(&url) {
            Ok(proxy) => builder.proxy(proxy),
            Err(err) => {
                log::warn!("Ignoring invalid proxy URL '{url}': {err}");
                builder
            }
        },
        None => builder,
    }
}

/// A reqwest client builder with the configured proxy already applied. Every
/// outbound client in the app should start here instead of
/// `reqwest::Client::builder()` so the proxy setting is honored everywhere.
pub(crate) fn client_builder() -> reqwest::ClientBuilder {
    apply_proxy(reqwest::Client::builder())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_defaults_scheme_and_clears_empty() {
        assert_eq!(normalize(None), None);
        assert_eq!(normalize(Some("   ".to_string())), None);
        assert_eq!(
            normalize(Some("127.0.0.1:7890".to_string())),
            Some("http://127.0.0.1:7890".to_string())
        );
        assert_eq!(
            normalize(Some("socks5://127.0.0.1:7891".to_string())),
            Some("socks5://127.0.0.1:7891".to_string())
        );
    }
}
