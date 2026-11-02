use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::time::Instant;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::{
    build_translation_attempts, diagnostics::append_translation_debug,
    format_translation_attempt_chain, stable_text_hash, translation, translation_fallback_enabled,
    AppDatabase, TranslationAttempt,
};

const TRANSLATION_PROMPT_VERSION: &str = "v1-basic-block-translation";

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StartDocumentTranslationInput {
    document_id: String,
    target_lang: String,
    provider: Option<String>,
    scope: Option<String>,
    page: Option<u32>,
    priority_page: Option<u32>,
    force_refresh: Option<bool>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GetPageTranslationInput {
    document_id: String,
    page_no: u32,
    target_lang: String,
    provider: Option<String>,
    provider_key: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CancelTranslationJobInput {
    job_id: String,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TranslationJobOutput {
    job_id: String,
    document_id: String,
    target_lang: String,
    provider_key: String,
    prompt_version: String,
    index_version: i64,
    status: String,
    total_blocks: u32,
    translated_blocks: u32,
    failed_blocks: u32,
    error: String,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TranslationJobEventOutput {
    job_id: String,
    document_id: String,
    target_lang: String,
    provider_key: String,
    prompt_version: String,
    index_version: i64,
    status: String,
    total_blocks: u32,
    translated_blocks: u32,
    failed_blocks: u32,
    phase: String,
    current_page: Option<u32>,
    error: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PageTranslationOutput {
    document_id: String,
    page_no: u32,
    target_lang: String,
    provider_key: String,
    prompt_version: String,
    status: String,
    total_blocks: u32,
    cached_blocks: u32,
    running_blocks: u32,
    failed_blocks: u32,
    missing_blocks: u32,
    blocks: Vec<TranslatedBlockOutput>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TranslatedBlockOutput {
    block_id: String,
    source_text: String,
    translated_text: String,
    bbox_list: serde_json::Value,
    order_index: u32,
    block_role: String,
    status: String,
    error: String,
}

#[derive(Clone)]
struct TranslationBlock {
    page_no: u32,
    block_id: String,
    block_index: u32,
    source_text: String,
    bbox_json: String,
    block_role: String,
    source_hash: String,
}

#[derive(Clone)]
struct TranslationJobPlan {
    document_id: String,
    target_lang: String,
    provider_key: String,
    prompt_version: String,
    index_version: i64,
    force_refresh: bool,
    blocks: Vec<TranslationBlock>,
}

struct TranslationJobKey {
    document_id: String,
    target_lang: String,
    provider_key: String,
    prompt_version: String,
    index_version: i64,
    source_version: String,
}

#[tauri::command]
pub(crate) async fn start_document_translation(
    input: StartDocumentTranslationInput,
    app: AppHandle,
    database: State<'_, AppDatabase>,
) -> Result<TranslationJobOutput, String> {
    crate::course_feature_gate("文档翻译")?;
    let started_at = Instant::now();
    append_translation_debug(
        &app,
        "start_document_translation:enter",
        serde_json::json!({
            "documentId": input.document_id.clone(),
            "targetLang": input.target_lang.clone(),
            "provider": input.provider.clone(),
            "scope": input.scope.clone(),
            "page": input.page,
            "priorityPage": input.priority_page,
            "forceRefresh": input.force_refresh,
        }),
    );
    let target_lang = normalize_target_lang(&input.target_lang)?;
    let selected_provider = input.provider.as_deref();
    let attempts = build_translation_attempts_for_provider(selected_provider, &database);
    let provider_key = provider_key_for_attempts(&attempts);
    let scope = input.scope.as_deref().unwrap_or("document");
    let page_filter = (scope == "page").then_some(input.page).flatten();
    let key = build_translation_job_key(
        &database,
        &input.document_id,
        &target_lang,
        &provider_key,
        page_filter,
    )?;

    if let Some(active) = read_active_job(&database, &key)? {
        append_translation_debug(
            &app,
            "start_document_translation:active_job",
            serde_json::json!({
                "jobId": active.job_id.clone(),
                "status": active.status.clone(),
                "elapsedMs": started_at.elapsed().as_millis(),
            }),
        );
        return Ok(active);
    }

    let job_id = create_translation_job(&database, &key)?;
    let output = read_translation_job(&database, &job_id)?;
    append_translation_debug(
        &app,
        "start_document_translation:job_created",
        serde_json::json!({
            "jobId": job_id.clone(),
            "elapsedMs": started_at.elapsed().as_millis(),
        }),
    );
    emit_translation_event(&app, &output);

    let task_app = app.clone();
    let task_input = input.clone();
    tauri::async_runtime::spawn(async move {
        run_translation_job(task_app, task_input, job_id, attempts, provider_key).await;
    });

    append_translation_debug(
        &app,
        "start_document_translation:return",
        serde_json::json!({
            "jobId": output.job_id.clone(),
            "status": output.status.clone(),
            "elapsedMs": started_at.elapsed().as_millis(),
        }),
    );
    Ok(output)
}

#[tauri::command]
pub(crate) fn get_page_translation(
    input: GetPageTranslationInput,
    database: State<'_, AppDatabase>,
) -> Result<PageTranslationOutput, String> {
    let target_lang = normalize_target_lang(&input.target_lang)?;
    let provider_key = input
        .provider_key
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .unwrap_or_else(|| resolve_provider_key(input.provider.as_deref(), &database));
    let conn = database
        .conn
        .lock()
        .map_err(|_| "SQLite lock was poisoned".to_string())?;
    let index_version = document_index_version(&conn, &input.document_id)?;

    let mut block_stmt = conn
        .prepare(
            "SELECT id, block_index, text, bbox_json, block_role
             FROM document_blocks
             WHERE document_id = ?1 AND page_no = ?2
             ORDER BY block_index",
        )
        .map_err(|err| format!("Failed to read document blocks: {err}"))?;
    let source_blocks = block_stmt
        .query_map(params![input.document_id, input.page_no], |row| {
            let block_id = row.get::<_, String>(0)?;
            let block_index = row.get::<_, u32>(1)?;
            let source_text = row.get::<_, String>(2)?;
            let bbox_json = row.get::<_, String>(3)?;
            let block_role = row.get::<_, String>(4)?;
            let source_hash =
                block_source_hash(&block_id, input.page_no, index_version, &source_text);
            Ok(TranslationBlock {
                page_no: input.page_no,
                block_id,
                block_index,
                source_text,
                bbox_json,
                block_role,
                source_hash,
            })
        })
        .map_err(|err| format!("Failed to read document blocks: {err}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| format!("Failed to decode document blocks: {err}"))?;
    drop(block_stmt);

    let mut blocks = Vec::with_capacity(source_blocks.len());
    let mut cached_blocks = 0u32;
    let mut failed_blocks = 0u32;
    let mut missing_blocks = 0u32;
    for block in source_blocks {
        let cached = conn
            .query_row(
                "SELECT translated_text, status, error
                 FROM translated_blocks
                 WHERE document_id = ?1
                   AND block_id = ?2
                   AND source_hash = ?3
                   AND target_lang = ?4
                   AND provider_key = ?5
                   AND prompt_version = ?6
                   AND index_version = ?7
                 LIMIT 1",
                params![
                    input.document_id,
                    block.block_id,
                    block.source_hash,
                    target_lang,
                    provider_key,
                    TRANSLATION_PROMPT_VERSION,
                    index_version
                ],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()
            .map_err(|err| format!("Failed to read translated block cache: {err}"))?;
        let (translated_text, status, error) = cached.unwrap_or_else(|| {
            (
                String::new(),
                "missing".to_string(),
                "Translation not available yet".to_string(),
            )
        });
        match status.as_str() {
            "succeeded" => cached_blocks = cached_blocks.saturating_add(1),
            "failed" => failed_blocks = failed_blocks.saturating_add(1),
            _ => missing_blocks = missing_blocks.saturating_add(1),
        }
        blocks.push(TranslatedBlockOutput {
            block_id: block.block_id,
            source_text: block.source_text,
            translated_text,
            bbox_list: parse_bbox_json(&block.bbox_json),
            order_index: block.block_index,
            block_role: block.block_role,
            status,
            error,
        });
    }

    let running_blocks = if has_active_job(&conn, &input.document_id, &target_lang, &provider_key)?
    {
        missing_blocks
    } else {
        0
    };
    let total_blocks = blocks.len() as u32;
    let status = if total_blocks == 0 {
        "empty"
    } else if failed_blocks > 0 && cached_blocks == 0 {
        "failed"
    } else if cached_blocks >= total_blocks {
        "succeeded"
    } else if running_blocks > 0 || cached_blocks > 0 {
        "partial"
    } else {
        "missing"
    }
    .to_string();

    Ok(PageTranslationOutput {
        document_id: input.document_id,
        page_no: input.page_no,
        target_lang,
        provider_key,
        prompt_version: TRANSLATION_PROMPT_VERSION.to_string(),
        status,
        total_blocks,
        cached_blocks,
        running_blocks,
        failed_blocks,
        missing_blocks,
        blocks,
    })
}

#[tauri::command]
pub(crate) fn cancel_translation_job(
    input: CancelTranslationJobInput,
    app: AppHandle,
    database: State<'_, AppDatabase>,
) -> Result<(), String> {
    let conn = database
        .conn
        .lock()
        .map_err(|_| "SQLite lock was poisoned".to_string())?;
    conn.execute(
        "UPDATE translation_jobs
         SET status = 'canceled',
             canceled_at = unixepoch(),
             updated_at = unixepoch()
         WHERE id = ?1 AND status IN ('queued', 'running')",
        params![input.job_id],
    )
    .map_err(|err| format!("Failed to cancel translation job: {err}"))?;
    drop(conn);
    if let Ok(job) = read_translation_job(&database, &input.job_id) {
        emit_translation_event(&app, &job);
    }
    Ok(())
}

async fn run_translation_job(
    app: AppHandle,
    input: StartDocumentTranslationInput,
    job_id: String,
    attempts: Vec<TranslationAttempt>,
    provider_key: String,
) {
    let started_at = Instant::now();
    append_translation_debug(
        &app,
        "run_translation_job:enter",
        serde_json::json!({
            "jobId": job_id,
            "documentId": input.document_id,
            "priorityPage": input.priority_page,
        }),
    );
    let database = app.state::<AppDatabase>();
    match mark_job_running(&database, &job_id) {
        Ok(true) => {}
        Ok(false) => {
            if let Ok(job) = read_translation_job(&database, &job_id) {
                emit_translation_event(&app, &job);
            }
            return;
        }
        Err(err) => {
            log::warn!("Failed to mark translation job running: {err}");
            return;
        }
    }
    if let Ok(job) = read_translation_job(&database, &job_id) {
        emit_translation_event(&app, &job);
    }

    let target_lang = match normalize_target_lang(&input.target_lang) {
        Ok(value) => value,
        Err(err) => {
            fail_job_and_emit(&app, &database, &job_id, &err);
            return;
        }
    };
    let scope = input.scope.as_deref().unwrap_or("document");
    let page_filter = (scope == "page").then_some(input.page).flatten();
    let force_refresh = input.force_refresh.unwrap_or(false);
    let plan = match build_translation_plan(
        &database,
        &input.document_id,
        &target_lang,
        &provider_key,
        page_filter,
        input.priority_page,
        force_refresh,
    ) {
        Ok(plan) => plan,
        Err(err) => {
            fail_job_and_emit(&app, &database, &job_id, &err);
            return;
        }
    };
    append_translation_debug(
        &app,
        "run_translation_job:plan_ready",
        serde_json::json!({
            "jobId": job_id,
            "blockCount": plan.blocks.len(),
            "forceRefresh": plan.force_refresh,
            "elapsedMs": started_at.elapsed().as_millis(),
        }),
    );
    if plan.blocks.is_empty() {
        fail_job_and_emit(
            &app,
            &database,
            &job_id,
            "This document has no indexed text blocks to translate",
        );
        return;
    }
    let cached_blocks = if plan.force_refresh {
        0
    } else {
        match count_cached_blocks(&database, &plan) {
            Ok(count) => count,
            Err(err) => {
                fail_job_and_emit(&app, &database, &job_id, &err);
                return;
            }
        }
    };
    if let Err(err) = update_job_totals(&database, &job_id, plan.blocks.len() as u32, cached_blocks)
    {
        fail_job_and_emit(&app, &database, &job_id, &err);
        return;
    }
    if let Ok(job) = read_translation_job(&database, &job_id) {
        emit_translation_event(&app, &job);
    }
    for block in plan.blocks.iter() {
        if is_job_canceled(&database, &job_id).unwrap_or(false) {
            return;
        }
        if !plan.force_refresh && is_block_cached(&database, &plan, block).unwrap_or(false) {
            continue;
        }
        if should_preserve_source_for_translation(block) {
            if let Err(err) = write_translated_block(&database, &plan, block, &block.source_text) {
                log::warn!("Failed to cache preserved translated block: {err}");
                let _ = increment_job_failed(&database, &job_id, &err);
            } else {
                let _ = increment_job_translated(&database, &job_id);
            }
            if let Ok(job) = read_translation_job(&database, &job_id) {
                emit_translation_event_for_page(&app, &job, "translating", Some(block.page_no));
            }
            continue;
        }

        let mut errors = Vec::new();
        let mut translated = None;
        for (index, attempt) in attempts.iter().enumerate() {
            match translation::translate_with_provider(
                &block.source_text,
                &plan.target_lang,
                &attempt.provider,
            )
            .await
            {
                Ok(text) => {
                    translated = Some((text, format_translation_attempt_chain(&attempts, index)));
                    break;
                }
                Err(err) => errors.push(format!("{}: {err}", attempt.provider.label())),
            }
        }

        match translated {
            Some((translated_text, _provider_label)) => {
                if is_job_canceled(&database, &job_id).unwrap_or(false) {
                    return;
                }
                if let Err(err) = write_translated_block(&database, &plan, block, &translated_text)
                {
                    log::warn!("Failed to cache translated block: {err}");
                    let _ = increment_job_failed(&database, &job_id, &err);
                } else {
                    let _ = increment_job_translated(&database, &job_id);
                }
            }
            None => {
                let error = if errors.is_empty() {
                    "No translation providers are available".to_string()
                } else {
                    errors.join("; ")
                };
                let _ = write_failed_block(&database, &plan, block, &error);
                let _ = increment_job_failed(&database, &job_id, &error);
            }
        }
        if let Ok(job) = read_translation_job(&database, &job_id) {
            emit_translation_event_for_page(&app, &job, "translating", Some(block.page_no));
        }
    }

    let _ = finalize_job(&database, &job_id);
    if let Ok(job) = read_translation_job(&database, &job_id) {
        emit_translation_event_for_page(&app, &job, "finished", None);
    }
    append_translation_debug(
        &app,
        "run_translation_job:finished",
        serde_json::json!({
            "jobId": job_id,
            "elapsedMs": started_at.elapsed().as_millis(),
        }),
    );
}

fn normalize_target_lang(lang: &str) -> Result<String, String> {
    let value = lang.trim();
    if value.is_empty() {
        Err("No target language selected".to_string())
    } else {
        Ok(value.to_string())
    }
}

fn should_preserve_source_for_translation(block: &TranslationBlock) -> bool {
    matches!(block.block_role.as_str(), "code" | "formula")
}

fn resolve_provider_key(provider: Option<&str>, database: &State<'_, AppDatabase>) -> String {
    let attempts = build_translation_attempts_for_provider(provider, database);
    provider_key_for_attempts(&attempts)
}

fn build_translation_attempts_for_provider(
    provider: Option<&str>,
    database: &State<'_, AppDatabase>,
) -> Vec<TranslationAttempt> {
    let selected = provider.unwrap_or("default");
    build_translation_attempts(selected, database, translation_fallback_enabled(database))
}

fn provider_key_for_attempts(attempts: &[TranslationAttempt]) -> String {
    let keys = attempts
        .iter()
        .map(|attempt| attempt.provider.cache_key())
        .collect::<Vec<_>>();
    if keys.is_empty() {
        "unavailable".to_string()
    } else {
        keys.join("||")
    }
}

fn build_translation_job_key(
    database: &State<'_, AppDatabase>,
    document_id: &str,
    target_lang: &str,
    provider_key: &str,
    page_filter: Option<u32>,
) -> Result<TranslationJobKey, String> {
    let conn = database
        .conn
        .lock()
        .map_err(|_| "SQLite lock was poisoned".to_string())?;
    let index_version = document_index_version(&conn, document_id)?;
    Ok(TranslationJobKey {
        document_id: document_id.to_string(),
        target_lang: target_lang.to_string(),
        provider_key: provider_key.to_string(),
        prompt_version: TRANSLATION_PROMPT_VERSION.to_string(),
        index_version,
        source_version: translation_source_version(index_version, page_filter),
    })
}

fn translation_source_version(index_version: i64, page_filter: Option<u32>) -> String {
    page_filter
        .map(|page_no| format!("index:{index_version}:page:{page_no}"))
        .unwrap_or_else(|| format!("index:{index_version}:document"))
}

fn build_translation_plan(
    database: &State<'_, AppDatabase>,
    document_id: &str,
    target_lang: &str,
    provider_key: &str,
    page_filter: Option<u32>,
    priority_page: Option<u32>,
    force_refresh: bool,
) -> Result<TranslationJobPlan, String> {
    let conn = database
        .conn
        .lock()
        .map_err(|_| "SQLite lock was poisoned".to_string())?;
    let index_version = document_index_version(&conn, document_id)?;
    let mut sql = String::from(
        "SELECT page_no, id, block_index, text, bbox_json, block_role
         FROM document_blocks
         WHERE document_id = ?1",
    );
    if page_filter.is_some() {
        sql.push_str(" AND page_no = ?2");
    }
    sql.push_str(" ORDER BY page_no, block_index");
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|err| format!("Failed to prepare translation block query: {err}"))?;
    let mut blocks = if let Some(page_no) = page_filter {
        stmt.query_map(params![document_id, page_no], |row| {
            decode_translation_block(row, index_version)
        })
        .map_err(|err| format!("Failed to read translation blocks: {err}"))?
        .collect::<Result<Vec<_>, _>>()
    } else {
        stmt.query_map(params![document_id], |row| {
            decode_translation_block(row, index_version)
        })
        .map_err(|err| format!("Failed to read translation blocks: {err}"))?
        .collect::<Result<Vec<_>, _>>()
    }
    .map_err(|err| format!("Failed to read translation blocks: {err}"))?;
    prioritize_translation_blocks(&mut blocks, priority_page);
    Ok(TranslationJobPlan {
        document_id: document_id.to_string(),
        target_lang: target_lang.to_string(),
        provider_key: provider_key.to_string(),
        prompt_version: TRANSLATION_PROMPT_VERSION.to_string(),
        index_version,
        force_refresh,
        blocks,
    })
}

fn prioritize_translation_blocks(blocks: &mut [TranslationBlock], priority_page: Option<u32>) {
    let Some(priority_page) = priority_page else {
        return;
    };
    blocks.sort_by_key(|block| {
        (
            if block.page_no == priority_page { 0 } else { 1 },
            if block.page_no >= priority_page { 0 } else { 1 },
            block.page_no,
            block.block_index,
        )
    });
}

fn decode_translation_block(
    row: &rusqlite::Row<'_>,
    index_version: i64,
) -> rusqlite::Result<TranslationBlock> {
    let page_no = row.get::<_, u32>(0)?;
    let block_id = row.get::<_, String>(1)?;
    let block_index = row.get::<_, u32>(2)?;
    let source_text = row.get::<_, String>(3)?;
    let bbox_json = row.get::<_, String>(4)?;
    let block_role = row.get::<_, String>(5)?;
    let source_hash = block_source_hash(&block_id, page_no, index_version, &source_text);
    Ok(TranslationBlock {
        page_no,
        block_id,
        block_index,
        source_text,
        bbox_json,
        block_role,
        source_hash,
    })
}

fn document_index_version(conn: &rusqlite::Connection, document_id: &str) -> Result<i64, String> {
    conn.query_row(
        "SELECT index_version FROM documents WHERE id = ?1",
        params![document_id],
        |row| row.get::<_, i64>(0),
    )
    .optional()
    .map_err(|err| format!("Failed to read document index version: {err}"))?
    .ok_or_else(|| "Unknown document".to_string())
}

fn block_source_hash(block_id: &str, page_no: u32, index_version: i64, text: &str) -> String {
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    stable_text_hash(&format!(
        "{block_id}\n{page_no}\n{index_version}\n{normalized}"
    ))
}

fn count_cached_blocks(
    database: &State<'_, AppDatabase>,
    plan: &TranslationJobPlan,
) -> Result<u32, String> {
    let conn = database
        .conn
        .lock()
        .map_err(|_| "SQLite lock was poisoned".to_string())?;
    let mut count = 0u32;
    for block in plan.blocks.iter() {
        if is_block_cached_with_conn(&conn, plan, block)? {
            count = count.saturating_add(1);
        }
    }
    Ok(count)
}

fn is_block_cached(
    database: &State<'_, AppDatabase>,
    plan: &TranslationJobPlan,
    block: &TranslationBlock,
) -> Result<bool, String> {
    let conn = database
        .conn
        .lock()
        .map_err(|_| "SQLite lock was poisoned".to_string())?;
    is_block_cached_with_conn(&conn, plan, block)
}

fn is_block_cached_with_conn(
    conn: &rusqlite::Connection,
    plan: &TranslationJobPlan,
    block: &TranslationBlock,
) -> Result<bool, String> {
    let exists: Option<i64> = conn
        .query_row(
            "SELECT 1
             FROM translated_blocks
             WHERE document_id = ?1
               AND block_id = ?2
               AND source_hash = ?3
               AND target_lang = ?4
               AND provider_key = ?5
               AND prompt_version = ?6
               AND index_version = ?7
               AND status = 'succeeded'
             LIMIT 1",
            params![
                plan.document_id,
                block.block_id,
                block.source_hash,
                plan.target_lang,
                plan.provider_key,
                plan.prompt_version,
                plan.index_version
            ],
            |row| row.get(0),
        )
        .optional()
        .map_err(|err| format!("Failed to read translated block cache: {err}"))?;
    Ok(exists.is_some())
}

fn read_active_job(
    database: &State<'_, AppDatabase>,
    key: &TranslationJobKey,
) -> Result<Option<TranslationJobOutput>, String> {
    let conn = database
        .conn
        .lock()
        .map_err(|_| "SQLite lock was poisoned".to_string())?;
    let job_id = conn
        .query_row(
            "SELECT id
             FROM translation_jobs
             WHERE document_id = ?1
               AND target_lang = ?2
               AND provider_key = ?3
               AND prompt_version = ?4
               AND index_version = ?5
               AND source_version = ?6
               AND status IN ('queued', 'running')
             LIMIT 1",
            params![
                key.document_id,
                key.target_lang,
                key.provider_key,
                key.prompt_version,
                key.index_version,
                key.source_version
            ],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|err| format!("Failed to read active translation job: {err}"))?;
    drop(conn);
    job_id
        .map(|id| read_translation_job(database, &id))
        .transpose()
}

fn create_translation_job(
    database: &State<'_, AppDatabase>,
    key: &TranslationJobKey,
) -> Result<String, String> {
    let job_id = format!(
        "translation-job-{}",
        stable_text_hash(&format!(
            "{}\n{}\n{}\n{}\n{}",
            key.document_id,
            key.target_lang,
            key.provider_key,
            key.index_version,
            key.source_version
        ))
    );
    let conn = database
        .conn
        .lock()
        .map_err(|_| "SQLite lock was poisoned".to_string())?;
    conn.execute(
        "INSERT INTO translation_jobs
           (id, document_id, target_lang, provider_key, prompt_version, index_version,
            source_version, status, total_blocks, translated_blocks, failed_blocks,
            error, created_at, updated_at, started_at, finished_at, canceled_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'queued', ?8, ?9, 0, '',
                 unixepoch(), unixepoch(), 0, 0, 0)
         ON CONFLICT(id) DO UPDATE SET
           status = CASE
             WHEN translation_jobs.status IN ('queued', 'running') THEN translation_jobs.status
             ELSE 'queued'
           END,
           total_blocks = excluded.total_blocks,
           translated_blocks = excluded.translated_blocks,
           failed_blocks = 0,
           error = '',
           updated_at = unixepoch(),
           finished_at = 0,
           canceled_at = 0",
        params![
            job_id,
            key.document_id,
            key.target_lang,
            key.provider_key,
            key.prompt_version,
            key.index_version,
            key.source_version,
            0u32,
            0u32
        ],
    )
    .map_err(|err| format!("Failed to create translation job: {err}"))?;
    Ok(job_id)
}

fn read_translation_job(
    database: &State<'_, AppDatabase>,
    job_id: &str,
) -> Result<TranslationJobOutput, String> {
    let conn = database
        .conn
        .lock()
        .map_err(|_| "SQLite lock was poisoned".to_string())?;
    conn.query_row(
        "SELECT id, document_id, target_lang, provider_key, prompt_version, index_version,
                status, total_blocks, translated_blocks, failed_blocks, error
         FROM translation_jobs
         WHERE id = ?1",
        params![job_id],
        |row| {
            Ok(TranslationJobOutput {
                job_id: row.get(0)?,
                document_id: row.get(1)?,
                target_lang: row.get(2)?,
                provider_key: row.get(3)?,
                prompt_version: row.get(4)?,
                index_version: row.get(5)?,
                status: row.get(6)?,
                total_blocks: row.get(7)?,
                translated_blocks: row.get(8)?,
                failed_blocks: row.get(9)?,
                error: row.get(10)?,
            })
        },
    )
    .map_err(|err| format!("Failed to read translation job: {err}"))
}

fn mark_job_running(database: &State<'_, AppDatabase>, job_id: &str) -> Result<bool, String> {
    let conn = database
        .conn
        .lock()
        .map_err(|_| "SQLite lock was poisoned".to_string())?;
    let updated = conn
        .execute(
            "UPDATE translation_jobs
         SET status = 'running',
             started_at = CASE WHEN started_at = 0 THEN unixepoch() ELSE started_at END,
             updated_at = unixepoch()
         WHERE id = ?1 AND status = 'queued'",
            params![job_id],
        )
        .map_err(|err| format!("Failed to mark translation job running: {err}"))?;
    Ok(updated > 0)
}

fn update_job_totals(
    database: &State<'_, AppDatabase>,
    job_id: &str,
    total_blocks: u32,
    cached_blocks: u32,
) -> Result<(), String> {
    let conn = database
        .conn
        .lock()
        .map_err(|_| "SQLite lock was poisoned".to_string())?;
    conn.execute(
        "UPDATE translation_jobs
         SET total_blocks = ?2,
             translated_blocks = ?3,
             failed_blocks = 0,
             error = '',
             updated_at = unixepoch()
         WHERE id = ?1 AND status = 'running'",
        params![job_id, total_blocks, cached_blocks],
    )
    .map_err(|err| format!("Failed to initialize translation progress: {err}"))?;
    Ok(())
}

fn mark_job_failed(
    database: &State<'_, AppDatabase>,
    job_id: &str,
    error: &str,
) -> Result<(), String> {
    let conn = database
        .conn
        .lock()
        .map_err(|_| "SQLite lock was poisoned".to_string())?;
    conn.execute(
        "UPDATE translation_jobs
         SET status = 'failed',
             error = ?2,
             updated_at = unixepoch(),
             finished_at = unixepoch()
         WHERE id = ?1",
        params![job_id, error],
    )
    .map_err(|err| format!("Failed to mark translation job failed: {err}"))?;
    Ok(())
}

fn fail_job_and_emit(
    app: &AppHandle,
    database: &State<'_, AppDatabase>,
    job_id: &str,
    error: &str,
) {
    if let Err(err) = mark_job_failed(database, job_id, error) {
        log::warn!("Failed to mark translation job failed: {err}");
        return;
    }
    if let Ok(job) = read_translation_job(database, job_id) {
        emit_translation_event(app, &job);
    }
}

fn is_job_canceled(database: &State<'_, AppDatabase>, job_id: &str) -> Result<bool, String> {
    let conn = database
        .conn
        .lock()
        .map_err(|_| "SQLite lock was poisoned".to_string())?;
    let status: Option<String> = conn
        .query_row(
            "SELECT status FROM translation_jobs WHERE id = ?1",
            params![job_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|err| format!("Failed to read translation job status: {err}"))?;
    Ok(status.as_deref() == Some("canceled"))
}

fn write_translated_block(
    database: &State<'_, AppDatabase>,
    plan: &TranslationJobPlan,
    block: &TranslationBlock,
    translated_text: &str,
) -> Result<(), String> {
    let conn = database
        .conn
        .lock()
        .map_err(|_| "SQLite lock was poisoned".to_string())?;
    conn.execute(
        "INSERT INTO translated_blocks
           (document_id, page_no, block_id, source_hash, target_lang, provider_key,
            prompt_version, index_version, source_text, translated_text, bbox_list,
            order_index, status, error, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 'succeeded', '',
                 unixepoch(), unixepoch())
         ON CONFLICT(document_id, block_id, source_hash, target_lang, provider_key, prompt_version, index_version)
         DO UPDATE SET
           page_no = excluded.page_no,
           source_text = excluded.source_text,
           translated_text = excluded.translated_text,
           bbox_list = excluded.bbox_list,
           order_index = excluded.order_index,
           status = 'succeeded',
           error = '',
           updated_at = unixepoch()",
        params![
            plan.document_id,
            block.page_no,
            block.block_id,
            block.source_hash,
            plan.target_lang,
            plan.provider_key,
            plan.prompt_version,
            plan.index_version,
            block.source_text,
            translated_text,
            block.bbox_json,
            block.block_index
        ],
    )
    .map_err(|err| format!("Failed to write translated block: {err}"))?;
    Ok(())
}

fn write_failed_block(
    database: &State<'_, AppDatabase>,
    plan: &TranslationJobPlan,
    block: &TranslationBlock,
    error: &str,
) -> Result<(), String> {
    let conn = database
        .conn
        .lock()
        .map_err(|_| "SQLite lock was poisoned".to_string())?;
    conn.execute(
        "INSERT INTO translated_blocks
           (document_id, page_no, block_id, source_hash, target_lang, provider_key,
            prompt_version, index_version, source_text, translated_text, bbox_list,
            order_index, status, error, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, '', ?10, ?11, 'failed', ?12,
                 unixepoch(), unixepoch())
         ON CONFLICT(document_id, block_id, source_hash, target_lang, provider_key, prompt_version, index_version)
         DO UPDATE SET
           page_no = excluded.page_no,
           source_text = excluded.source_text,
           bbox_list = excluded.bbox_list,
           order_index = excluded.order_index,
           status = 'failed',
           error = excluded.error,
           updated_at = unixepoch()",
        params![
            plan.document_id,
            block.page_no,
            block.block_id,
            block.source_hash,
            plan.target_lang,
            plan.provider_key,
            plan.prompt_version,
            plan.index_version,
            block.source_text,
            block.bbox_json,
            block.block_index,
            error
        ],
    )
    .map_err(|err| format!("Failed to write failed translated block: {err}"))?;
    Ok(())
}

fn increment_job_translated(database: &State<'_, AppDatabase>, job_id: &str) -> Result<(), String> {
    let conn = database
        .conn
        .lock()
        .map_err(|_| "SQLite lock was poisoned".to_string())?;
    conn.execute(
        "UPDATE translation_jobs
         SET translated_blocks = MIN(total_blocks, translated_blocks + 1),
             updated_at = unixepoch()
         WHERE id = ?1 AND status = 'running'",
        params![job_id],
    )
    .map_err(|err| format!("Failed to update translation progress: {err}"))?;
    Ok(())
}

fn increment_job_failed(
    database: &State<'_, AppDatabase>,
    job_id: &str,
    error: &str,
) -> Result<(), String> {
    let conn = database
        .conn
        .lock()
        .map_err(|_| "SQLite lock was poisoned".to_string())?;
    conn.execute(
        "UPDATE translation_jobs
         SET failed_blocks = failed_blocks + 1,
             error = ?2,
             updated_at = unixepoch()
         WHERE id = ?1 AND status = 'running'",
        params![job_id, error],
    )
    .map_err(|err| format!("Failed to update translation failure count: {err}"))?;
    Ok(())
}

fn finalize_job(database: &State<'_, AppDatabase>, job_id: &str) -> Result<(), String> {
    let conn = database
        .conn
        .lock()
        .map_err(|_| "SQLite lock was poisoned".to_string())?;
    conn.execute(
        "UPDATE translation_jobs
         SET status = CASE
             WHEN failed_blocks > 0 AND translated_blocks = 0 THEN 'failed'
             WHEN failed_blocks > 0 THEN 'partial'
             ELSE 'succeeded'
           END,
           updated_at = unixepoch(),
           finished_at = unixepoch()
         WHERE id = ?1 AND status = 'running'",
        params![job_id],
    )
    .map_err(|err| format!("Failed to finalize translation job: {err}"))?;
    Ok(())
}

fn has_active_job(
    conn: &rusqlite::Connection,
    document_id: &str,
    target_lang: &str,
    provider_key: &str,
) -> Result<bool, String> {
    conn.query_row(
        "SELECT 1
         FROM translation_jobs
         WHERE document_id = ?1
           AND target_lang = ?2
           AND provider_key = ?3
           AND status IN ('queued', 'running')
         LIMIT 1",
        params![document_id, target_lang, provider_key],
        |row| row.get::<_, i64>(0),
    )
    .optional()
    .map_err(|err| format!("Failed to read active translation job: {err}"))
    .map(|value| value.is_some())
}

fn parse_bbox_json(value: &str) -> serde_json::Value {
    serde_json::from_str(value).unwrap_or_else(|_| serde_json::Value::Array(Vec::new()))
}

fn emit_translation_event(app: &AppHandle, job: &TranslationJobOutput) {
    emit_translation_event_for_page(app, job, job.status.as_str(), None);
}

fn emit_translation_event_for_page(
    app: &AppHandle,
    job: &TranslationJobOutput,
    phase: &str,
    current_page: Option<u32>,
) {
    let event = TranslationJobEventOutput {
        job_id: job.job_id.clone(),
        document_id: job.document_id.clone(),
        target_lang: job.target_lang.clone(),
        provider_key: job.provider_key.clone(),
        prompt_version: job.prompt_version.clone(),
        index_version: job.index_version,
        status: job.status.clone(),
        total_blocks: job.total_blocks,
        translated_blocks: job.translated_blocks,
        failed_blocks: job.failed_blocks,
        phase: phase.to_string(),
        current_page,
        error: job.error.clone(),
    };
    append_translation_debug(
        app,
        "emit_translation_event",
        serde_json::json!({
            "jobId": job.job_id,
            "documentId": job.document_id,
            "status": job.status,
            "phase": phase,
            "currentPage": current_page,
            "totalBlocks": job.total_blocks,
            "translatedBlocks": job.translated_blocks,
            "failedBlocks": job.failed_blocks,
        }),
    );
    if let Err(err) = app.emit("markdownnote://translation-job", event) {
        log::warn!("Failed to emit translation job event: {err}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(page_no: u32, block_index: u32) -> TranslationBlock {
        TranslationBlock {
            page_no,
            block_id: format!("p{page_no}-b{block_index}"),
            block_index,
            source_text: String::new(),
            bbox_json: "[]".to_string(),
            block_role: "body".to_string(),
            source_hash: String::new(),
        }
    }

    #[test]
    fn priority_page_is_translated_before_document_tail() {
        let mut blocks = vec![
            block(1, 0),
            block(2, 0),
            block(3, 0),
            block(4, 0),
            block(5, 0),
        ];
        prioritize_translation_blocks(&mut blocks, Some(3));
        let pages = blocks
            .into_iter()
            .map(|block| block.page_no)
            .collect::<Vec<_>>();
        assert_eq!(pages, vec![3, 4, 5, 1, 2]);
    }
}
