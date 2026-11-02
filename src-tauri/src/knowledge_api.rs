//! The knowledge base as a local API: a resident MCP server external harnesses
//! (Claude Code, Codex, any MCP client) can point at.
//!
//! This is the same server the in-app local-agent path uses, held open with a
//! stable address instead of started per turn — see `local_agent::mcp_server`
//! for the scope split that makes it safe to hand to a program rather than a
//! model.
//!
//! Off by default, and deliberately so: while it runs, any process on this
//! machine holding the token can read the entire knowledge base without going
//! through the app. It binds loopback only, exposes read-only tools, and the
//! token can be rotated to cut off anything already configured.

use std::sync::Mutex;

use rusqlite::Connection;
use tauri::State;

use crate::local_agent::mcp_server::{self, McpScope, RunningMcpServer};
use crate::AppDatabase;

pub(crate) const ENABLED_SETTING: &str = "mcp_server_enabled";
pub(crate) const PORT_SETTING: &str = "mcp_server_port";
pub(crate) const TOKEN_SETTING: &str = "mcp_server_token";

/// Default listen port. Fixed rather than ephemeral so an external client can be
/// configured once; a clash is reported instead of silently moving.
pub(crate) const DEFAULT_PORT: u16 = 37650;

/// Holds the running server. Dropping the handle shuts it down, so replacing the
/// contents of this Option is how the service is stopped or restarted.
#[derive(Default)]
pub(crate) struct KnowledgeApiState {
    server: Mutex<Option<RunningMcpServer>>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct KnowledgeApiSettings {
    pub enabled: bool,
    pub port: u16,
    pub token: String,
    /// The endpoint to configure in an external client; empty when not running.
    pub url: String,
    /// Whether the listener is actually up — `enabled` is intent, this is fact.
    /// They diverge when the port was taken at startup.
    pub running: bool,
    /// Why the last start failed, if it did.
    pub error: String,
}

fn configured_port(conn: &Connection) -> u16 {
    crate::load_app_setting(conn, PORT_SETTING)
        .ok()
        .flatten()
        .and_then(|value| value.trim().parse::<u16>().ok())
        .filter(|port| *port > 0)
        .unwrap_or(DEFAULT_PORT)
}

fn configured_enabled(conn: &Connection) -> bool {
    crate::load_app_setting(conn, ENABLED_SETTING)
        .ok()
        .flatten()
        .map(|value| value.trim() == "1")
        .unwrap_or(false)
}

/// The persisted bearer token, generating and storing one on first use so an
/// external client keeps working across restarts.
fn ensure_token(conn: &Connection) -> Result<String, String> {
    if let Some(existing) = crate::load_app_setting(conn, TOKEN_SETTING)?
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    {
        return Ok(existing);
    }
    let token = mcp_server::random_token();
    crate::save_app_setting(conn, TOKEN_SETTING, &token)?;
    Ok(token)
}

/// Start the resident server, replacing any already running. Returns its URL.
async fn start(
    db_path: std::path::PathBuf,
    port: u16,
    token: String,
    state: &KnowledgeApiState,
) -> Result<String, String> {
    crate::course_feature_gate("知识库 MCP 端点")?;
    // Drop the old handle first: it holds the port, so starting before stopping
    // would fail against ourselves on an unchanged port.
    stop(state);
    let server = mcp_server::start_scoped_mcp_server(
        db_path,
        McpScope::Library,
        // Ignored under Library scope, which forces web tools off regardless.
        false,
        port,
        Some(token),
    )
    .await?;
    let url = server.url.clone();
    if let Ok(mut slot) = state.server.lock() {
        *slot = Some(server);
    }
    Ok(url)
}

pub(crate) fn stop(state: &KnowledgeApiState) {
    if let Ok(mut slot) = state.server.lock() {
        // Drop shuts the listener down.
        *slot = None;
    }
}

fn running_url(state: &KnowledgeApiState) -> Option<String> {
    state
        .server
        .lock()
        .ok()
        .and_then(|slot| slot.as_ref().map(|server| server.url.clone()))
}

fn settings_snapshot(
    conn: &Connection,
    state: &KnowledgeApiState,
    error: String,
) -> Result<KnowledgeApiSettings, String> {
    let url = running_url(state);
    Ok(KnowledgeApiSettings {
        enabled: configured_enabled(conn),
        port: configured_port(conn),
        token: ensure_token(conn)?,
        running: url.is_some(),
        url: url.unwrap_or_default(),
        error,
    })
}

// 课程展示版：原来的 `start_if_enabled` 会在启动时按旧设置拉起 MCP 端点，
// 让本机任何持有 token 的进程都能读取整个知识库。课程版不需要该能力，
// 启动链路已完全不调用它；设置读写仍在，`start` 一律返回停用错误。

#[cfg(test)]
mod tests {
    use super::*;

    fn settings_conn() -> Connection {
        let conn = Connection::open_in_memory().expect("open");
        conn.execute_batch(
            "CREATE TABLE app_settings (key TEXT PRIMARY KEY, value TEXT NOT NULL,
                 updated_at INTEGER NOT NULL DEFAULT 0);",
        )
        .expect("schema");
        conn
    }

    /// The security-relevant default: a fresh install does not expose the
    /// knowledge base until the user turns it on.
    #[test]
    fn the_service_is_off_until_explicitly_enabled() {
        let conn = settings_conn();
        assert!(!configured_enabled(&conn));
        crate::save_app_setting(&conn, ENABLED_SETTING, "1").expect("save");
        assert!(configured_enabled(&conn));
        // Anything but "1" is off, so a malformed value fails closed.
        crate::save_app_setting(&conn, ENABLED_SETTING, "yes").expect("save");
        assert!(!configured_enabled(&conn));
    }

    #[test]
    fn port_falls_back_to_the_default_for_missing_or_bad_values() {
        let conn = settings_conn();
        assert_eq!(configured_port(&conn), DEFAULT_PORT);
        crate::save_app_setting(&conn, PORT_SETTING, "8123").expect("save");
        assert_eq!(configured_port(&conn), 8123);
        // 0 would ask the OS for a random port, defeating a stable address.
        crate::save_app_setting(&conn, PORT_SETTING, "0").expect("save");
        assert_eq!(configured_port(&conn), DEFAULT_PORT);
        crate::save_app_setting(&conn, PORT_SETTING, "not-a-port").expect("save");
        assert_eq!(configured_port(&conn), DEFAULT_PORT);
    }

    /// The token has to survive restarts, or every relaunch would silently break
    /// whatever the user configured in their external client.
    #[test]
    fn the_token_is_generated_once_and_then_reused() {
        let conn = settings_conn();
        let first = ensure_token(&conn).expect("generate");
        assert_eq!(first.len(), 32, "128-bit hex token");
        assert_eq!(ensure_token(&conn).expect("reuse"), first);
        // Rotation replaces it; the old one stops working because the server is
        // restarted with the new value.
        crate::save_app_setting(&conn, TOKEN_SETTING, &mcp_server::random_token()).expect("save");
        assert_ne!(ensure_token(&conn).expect("rotated"), first);
    }
}

#[tauri::command]
pub(crate) async fn load_knowledge_api_settings(
    database: State<'_, AppDatabase>,
    state: State<'_, KnowledgeApiState>,
) -> Result<KnowledgeApiSettings, String> {
    let conn = database
        .conn
        .lock()
        .map_err(|_| "SQLite lock was poisoned".to_string())?;
    settings_snapshot(&conn, &state, String::new())
}

/// Turn the service on or off, and/or move it to another port. Applies
/// immediately — the caller should not have to restart the app to test it.
#[tauri::command]
pub(crate) async fn save_knowledge_api_settings(
    enabled: bool,
    port: u16,
    database: State<'_, AppDatabase>,
    state: State<'_, KnowledgeApiState>,
    db_path: State<'_, crate::DatabasePath>,
) -> Result<KnowledgeApiSettings, String> {
    let port = if port == 0 { DEFAULT_PORT } else { port };
    let token = {
        let conn = database
            .conn
            .lock()
            .map_err(|_| "SQLite lock was poisoned".to_string())?;
        crate::save_app_setting(&conn, ENABLED_SETTING, if enabled { "1" } else { "0" })?;
        crate::save_app_setting(&conn, PORT_SETTING, &port.to_string())?;
        ensure_token(&conn)?
    };

    let mut error = String::new();
    if enabled {
        if let Err(err) = start(db_path.0.clone(), port, token, &state).await {
            // Keep the user's intent recorded but report the failure, rather than
            // flipping the toggle back and hiding why.
            error = err;
        }
    } else {
        stop(&state);
    }

    let conn = database
        .conn
        .lock()
        .map_err(|_| "SQLite lock was poisoned".to_string())?;
    settings_snapshot(&conn, &state, error)
}

/// Issue a new token, cutting off any client still using the old one. Restarts
/// the listener so the change takes effect immediately.
#[tauri::command]
pub(crate) async fn rotate_knowledge_api_token(
    database: State<'_, AppDatabase>,
    state: State<'_, KnowledgeApiState>,
    db_path: State<'_, crate::DatabasePath>,
) -> Result<KnowledgeApiSettings, String> {
    let (enabled, port, token) = {
        let conn = database
            .conn
            .lock()
            .map_err(|_| "SQLite lock was poisoned".to_string())?;
        let token = mcp_server::random_token();
        crate::save_app_setting(&conn, TOKEN_SETTING, &token)?;
        (configured_enabled(&conn), configured_port(&conn), token)
    };

    let mut error = String::new();
    if enabled {
        if let Err(err) = start(db_path.0.clone(), port, token, &state).await {
            error = err;
        }
    }

    let conn = database
        .conn
        .lock()
        .map_err(|_| "SQLite lock was poisoned".to_string())?;
    settings_snapshot(&conn, &state, error)
}
