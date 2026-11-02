//! 课程展示版：笔记正文搜索与简单平级标签。
//!
//! 只依赖既有 `documents` 表（独立笔记的正文存放在 `body_md`，`content_type`
//! 为 `note` / `markdown` / `text`），新增两张本模块私有的表：
//! `course_tags`（标签的规范名称）与 `course_note_tags`（笔记与标签的关联）。
//! 不改动 `notes` 批注表、不改动 `documents` 列，旧数据库可直接打开。
//!
//! 搜索是 SQLite 上的字面子串匹配（`instr(lower(..), ?)`），不依赖模型、向量库
//! 或 AI 知识图谱；小规模演示直接查正文即可，不宣称十万篇级别的性能指标。
//!
//! 四个 Tauri 命令的对外约定（命令名、外层参数、返回字段、错误前缀）见交付包中的
//! 《前端对接说明.md》，与《开发提示词.md》B-03 一致。

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::AppDatabase;

/// 参与笔记搜索与标签的正文类型。`web` 网页剪辑、PDF 与 Office 文件不在此列：
/// 标签只服务于可编辑的笔记类来源。
const SUPPORTED_CONTENT_TYPES: [&str; 3] = ["note", "markdown", "text"];

/// 错误前缀：前端按前缀区分"输入不合法 / 找不到 / 不能打标签 / 存储故障"。
pub(crate) const ERR_INVALID_INPUT: &str = "COURSE_INVALID_INPUT";
pub(crate) const ERR_NOTE_NOT_FOUND: &str = "COURSE_NOTE_NOT_FOUND";
pub(crate) const ERR_NOTE_NOT_EDITABLE: &str = "COURSE_NOTE_NOT_EDITABLE";
pub(crate) const ERR_STORAGE: &str = "COURSE_STORAGE_ERROR";

/// `query` 上限，按 Unicode 标量（字符）计数。
const MAX_QUERY_CHARS: usize = 100;
/// 摘要上限，含省略号。
const MAX_SNIPPET_CHARS: usize = 120;
/// 摘要为命中位置预留的前文长度，命中点不会顶在摘要开头。
const SNIPPET_CONTEXT_CHARS: usize = 20;
const DEFAULT_LIMIT: i64 = 50;
const MAX_LIMIT: i64 = 100;
const MAX_TAGS_PER_NOTE: usize = 10;
const MAX_TAG_CHARS: usize = 20;

// ---------------------------------------------------------------------------
// 错误文本
// ---------------------------------------------------------------------------

fn invalid_input(message: impl std::fmt::Display) -> String {
    format!("{}: {message}", ERR_INVALID_INPUT)
}

fn note_not_found(document_id: &str) -> String {
    format!("{}: 找不到笔记 {document_id}", ERR_NOTE_NOT_FOUND)
}

fn note_not_editable(content_type: &str) -> String {
    format!(
        "{}: content_type={content_type} 的文档不支持笔记标签",
        ERR_NOTE_NOT_EDITABLE
    )
}

fn storage_error(what: &str, err: impl std::fmt::Display) -> String {
    format!("{}: {what}：{err}", ERR_STORAGE)
}

// ---------------------------------------------------------------------------
// 建表
// ---------------------------------------------------------------------------

/// 两张表都是 `IF NOT EXISTS`，可以重复初始化；旧数据库打开时补齐，不需要迁移
/// `documents`。`course_note_tags` 对 `documents(id)` 有级联删除，删除笔记时关联
/// 自动清理，其他笔记的标签与标签记录本身不受影响。
const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS course_tags (
  id TEXT PRIMARY KEY,
  -- 首次保存时的规范名称（之后同名标签复用它，不再改名）。
  name TEXT NOT NULL,
  -- 规范化键：name 的 ASCII 小写，唯一，保证 'Course' 与 'course' 是同一个标签。
  name_lower TEXT NOT NULL UNIQUE,
  created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS course_note_tags (
  document_id TEXT NOT NULL,
  tag_id TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  PRIMARY KEY (document_id, tag_id),
  FOREIGN KEY(document_id) REFERENCES documents(id) ON DELETE CASCADE,
  FOREIGN KEY(tag_id) REFERENCES course_tags(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_course_note_tags_tag
  ON course_note_tags(tag_id);
";

/// 启动时调用一次（`lib.rs` 的 `setup`），之后命令即可直接使用两张表。
pub(crate) fn ensure_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(SCHEMA)
        .map_err(|err| storage_error("初始化笔记标签表失败", err))
}

// ---------------------------------------------------------------------------
// 对外数据结构
// ---------------------------------------------------------------------------

/// `course_search_notes` 的 `input` 对象。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CourseSearchInput {
    #[serde(default)]
    query: String,
    #[serde(default)]
    tag: Option<String>,
    #[serde(default)]
    limit: Option<i64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CourseSearchItem {
    document_id: String,
    title: String,
    snippet: String,
    content_type: String,
    tags: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CourseSearchOutput {
    items: Vec<CourseSearchItem>,
    total: usize,
    truncated: bool,
}

/// `course_save_note_tags` 的 `input` 对象。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CourseSaveTagsInput {
    document_id: String,
    #[serde(default)]
    tags: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CourseTagSummary {
    name: String,
    note_count: i64,
}

// ---------------------------------------------------------------------------
// 搜索
// ---------------------------------------------------------------------------

/// 三个条件共用：只查笔记类正文；`?1` 为小写化后的关键词（空串=不限制）；
/// `?2` 为小写化后的标签名（空串=不限制标签）。用户输入全部走参数绑定。
const SEARCH_SCOPE: &str = "
  FROM documents d
 WHERE d.content_type IN ('note', 'markdown', 'text')
   AND (
        ?1 = ''
        OR instr(lower(d.title), ?1) > 0
        OR instr(lower(COALESCE(d.body_md, '')), ?1) > 0
   )
   AND (
        ?2 = ''
        OR EXISTS (
             SELECT 1
               FROM course_note_tags nt
               JOIN course_tags t ON t.id = nt.tag_id
              WHERE nt.document_id = d.id
                AND t.name_lower = ?2
           )
   )";

fn search_notes(
    conn: &Connection,
    input: CourseSearchInput,
) -> Result<CourseSearchOutput, String> {
    let query = input.query.trim();
    if query.chars().count() > MAX_QUERY_CHARS {
        return Err(invalid_input(format!(
            "搜索词最多 {MAX_QUERY_CHARS} 个字符"
        )));
    }
    let limit = match input.limit {
        None => DEFAULT_LIMIT,
        Some(limit) if (1..=MAX_LIMIT).contains(&limit) => limit,
        Some(_) => {
            return Err(invalid_input(format!(
                "limit 必须是 1 到 {MAX_LIMIT} 之间的整数"
            )))
        }
    };
    // SQLite 的内建 lower() 只折叠 ASCII 大小写，这里用同样的规则生成关键词，
    // 两边一致：中文按字面匹配，英文字母忽略大小写。
    let needle = query.to_ascii_lowercase();
    let tag_key = input
        .tag
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_ascii_lowercase())
        .unwrap_or_default();

    let total: i64 = conn
        .query_row(
            &format!("SELECT COUNT(*) {SEARCH_SCOPE}"),
            params![&needle, &tag_key],
            |row| row.get(0),
        )
        .map_err(|err| storage_error("统计匹配笔记失败", err))?;

    let mut stmt = conn
        .prepare(&format!(
            "SELECT d.id, d.title, COALESCE(d.body_md, ''), d.content_type
             {SEARCH_SCOPE}
             ORDER BY d.updated_at DESC, d.id ASC
             LIMIT ?3"
        ))
        .map_err(|err| storage_error("准备笔记搜索失败", err))?;

    let rows = stmt
        .query_map(params![&needle, &tag_key, limit], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .map_err(|err| storage_error("执行笔记搜索失败", err))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| storage_error("读取笔记搜索结果失败", err))?;
    drop(stmt);

    let mut items = Vec::with_capacity(rows.len());
    for (document_id, title, body_md, content_type) in rows {
        let tags = tags_for_document(conn, &document_id)?;
        items.push(CourseSearchItem {
            document_id,
            title,
            snippet: snippet_for(&body_md, &needle),
            content_type,
            tags,
        });
    }

    let total = usize::try_from(total.max(0)).unwrap_or(usize::MAX);
    Ok(CourseSearchOutput {
        truncated: total > items.len(),
        total,
        items,
    })
}

/// 单篇笔记的标签名，按 ASCII 不区分大小写排序（同名时按原名称）。
fn tags_for_document(conn: &Connection, document_id: &str) -> Result<Vec<String>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT t.name
               FROM course_note_tags nt
               JOIN course_tags t ON t.id = nt.tag_id
              WHERE nt.document_id = ?1
              ORDER BY t.name_lower ASC, t.name ASC",
        )
        .map_err(|err| storage_error("准备标签查询失败", err))?;
    let tags = stmt
        .query_map(params![document_id], |row| row.get::<_, String>(0))
        .map_err(|err| storage_error("读取笔记标签失败", err))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| storage_error("读取笔记标签失败", err))?;
    Ok(tags)
}

// ---------------------------------------------------------------------------
// 摘要
// ---------------------------------------------------------------------------

/// 把正文里的换行与连续空白压成一个空格，首尾不留空白：摘要是一行文本。
fn collapse_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut pending_space = false;
    for ch in text.chars() {
        if ch.is_whitespace() {
            pending_space = !out.is_empty();
            continue;
        }
        if pending_space {
            out.push(' ');
            pending_space = false;
        }
        out.push(ch);
    }
    out
}

/// 正文为空返回空串；正文命中关键词时把窗口移到命中位置附近（最多
/// [`MAX_SNIPPET_CHARS`] 个字符，含省略号），否则取正文开头。
fn snippet_for(body: &str, needle_lower: &str) -> String {
    let collapsed = collapse_whitespace(body);
    if collapsed.is_empty() {
        return String::new();
    }
    if needle_lower.is_empty() {
        return take_window(&collapsed, 0, MAX_SNIPPET_CHARS);
    }
    // to_ascii_lowercase 不改变字节长度，所以这里的字节下标在原文里依然合法。
    let lowered = collapsed.to_ascii_lowercase();
    let Some(byte_index) = lowered.find(needle_lower) else {
        return take_window(&collapsed, 0, MAX_SNIPPET_CHARS);
    };
    let char_index = collapsed[..byte_index].chars().count();
    let start = char_index.saturating_sub(SNIPPET_CONTEXT_CHARS);
    take_window(&collapsed, start, MAX_SNIPPET_CHARS)
}

/// 从 `start_char` 起取最多 `limit_chars` 个字符；被截断的一侧补省略号，
/// 结果长度不超过 `limit_chars`。
fn take_window(text: &str, start_char: usize, limit_chars: usize) -> String {
    let total_chars = text.chars().count();
    if total_chars <= limit_chars {
        return text.to_string();
    }
    if limit_chars == 0 {
        return String::new();
    }
    let start = start_char.min(total_chars.saturating_sub(1));
    let prefix = start > 0;
    let start_byte = text
        .char_indices()
        .nth(start)
        .map(|(index, _)| index)
        .unwrap_or(text.len());
    let tail = &text[start_byte..];
    let tail_chars = tail.chars().count();
    let mut budget = limit_chars - usize::from(prefix);
    let suffix = tail_chars > budget;
    if suffix {
        budget = budget.saturating_sub(1);
    }
    let mut out = String::new();
    if prefix {
        out.push('…');
    }
    out.extend(tail.chars().take(budget));
    if suffix {
        out.push('…');
    }
    out
}

// ---------------------------------------------------------------------------
// 标签读写
// ---------------------------------------------------------------------------

/// 文档必须存在且是笔记类正文，否则给出可读的错误。
fn require_supported_document(conn: &Connection, document_id: &str) -> Result<(), String> {
    let content_type: Option<String> = conn
        .query_row(
            "SELECT content_type FROM documents WHERE id = ?1",
            params![document_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|err| storage_error("读取文档类型失败", err))?;
    let Some(content_type) = content_type else {
        return Err(note_not_found(document_id));
    };
    if !SUPPORTED_CONTENT_TYPES.contains(&content_type.as_str()) {
        return Err(note_not_editable(&content_type));
    }
    Ok(())
}

/// 校验并规范化标签：去首尾空白 → 每条校验长度与控制字符 → 按 ASCII 不区分
/// 大小写去重（保留首次出现的写法）→ 校验数量。
fn normalize_tags(raw: &[String]) -> Result<Vec<String>, String> {
    let mut canonical: Vec<String> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for value in raw {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(invalid_input("标签不能为空"));
        }
        if trimmed.chars().count() > MAX_TAG_CHARS {
            return Err(invalid_input(format!(
                "单个标签最多 {MAX_TAG_CHARS} 个字符：{trimmed}"
            )));
        }
        if trimmed.chars().any(char::is_control) {
            return Err(invalid_input(format!(
                "标签不能包含控制字符：{trimmed}"
            )));
        }
        let key = trimmed.to_ascii_lowercase();
        if seen.iter().any(|existing| existing == &key) {
            continue;
        }
        seen.push(key);
        canonical.push(trimmed.to_string());
    }
    if canonical.len() > MAX_TAGS_PER_NOTE {
        return Err(invalid_input(format!(
            "每篇笔记最多 {MAX_TAGS_PER_NOTE} 个标签"
        )));
    }
    Ok(canonical)
}

fn new_tag_id() -> String {
    format!("tag-{:016x}{:016x}", rand::random::<u64>(), rand::random::<u64>())
}

fn load_note_tags(conn: &Connection, document_id: &str) -> Result<Vec<String>, String> {
    let document_id = document_id.trim();
    if document_id.is_empty() {
        return Err(invalid_input("documentId 不能为空"));
    }
    require_supported_document(conn, document_id)?;
    tags_for_document(conn, document_id)
}

/// 全量替换一篇笔记的标签：先清空关联再逐条写入，任一环节失败整笔回滚。
/// 返回实际保存的规范名称（同名标签沿用首次保存的写法）。
fn save_note_tags(
    conn: &Connection,
    input: CourseSaveTagsInput,
) -> Result<Vec<String>, String> {
    let document_id = input.document_id.trim();
    if document_id.is_empty() {
        return Err(invalid_input("documentId 不能为空"));
    }
    let canonical = normalize_tags(&input.tags)?;
    require_supported_document(conn, document_id)?;

    let tx = conn
        .unchecked_transaction()
        .map_err(|err| storage_error("开启标签事务失败", err))?;
    tx.execute(
        "DELETE FROM course_note_tags WHERE document_id = ?1",
        params![document_id],
    )
    .map_err(|err| storage_error("清理旧标签关联失败", err))?;

    let mut saved: Vec<String> = Vec::new();
    for name in canonical {
        let name_lower = name.to_ascii_lowercase();
        let existing: Option<(String, String)> = tx
            .query_row(
                "SELECT id, name FROM course_tags WHERE name_lower = ?1",
                params![&name_lower],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|err| storage_error("查询标签失败", err))?;
        let (tag_id, display_name) = match existing {
            Some((id, stored_name)) => (id, stored_name),
            None => {
                let id = new_tag_id();
                tx.execute(
                    "INSERT INTO course_tags (id, name, name_lower, created_at)
                     VALUES (?1, ?2, ?3, unixepoch())",
                    params![&id, &name, &name_lower],
                )
                .map_err(|err| storage_error("写入标签失败", err))?;
                (id, name)
            }
        };
        tx.execute(
            "INSERT OR IGNORE INTO course_note_tags (document_id, tag_id, created_at)
             VALUES (?1, ?2, unixepoch())",
            params![document_id, &tag_id],
        )
        .map_err(|err| storage_error("写入笔记标签关联失败", err))?;
        if !saved.iter().any(|value| value == &display_name) {
            saved.push(display_name);
        }
    }
    tx.commit()
        .map_err(|err| storage_error("提交标签事务失败", err))?;
    Ok(saved)
}

/// 只返回笔记类正文实际使用的标签，`noteCount` 按笔记去重。
fn list_tags(conn: &Connection) -> Result<Vec<CourseTagSummary>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT t.name, COUNT(DISTINCT nt.document_id) AS note_count
               FROM course_tags t
               JOIN course_note_tags nt ON nt.tag_id = t.id
               JOIN documents d ON d.id = nt.document_id
              WHERE d.content_type IN ('note', 'markdown', 'text')
              GROUP BY t.id, t.name, t.name_lower
              ORDER BY t.name_lower ASC, t.name ASC",
        )
        .map_err(|err| storage_error("准备标签列表失败", err))?;
    let tags = stmt
        .query_map([], |row| {
            Ok(CourseTagSummary {
                name: row.get(0)?,
                note_count: row.get(1)?,
            })
        })
        .map_err(|err| storage_error("读取标签列表失败", err))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| storage_error("读取标签列表失败", err))?;
    Ok(tags)
}

/// 删除笔记时清理它的标签关联。`documents` 的级联删除本来就会做同样的事，
/// 这里显式再清理一次，避免在临时关闭外键的连接上留下悬挂行；标签记录与其他
/// 笔记的关联保持不变。
pub(crate) fn delete_associations(conn: &Connection, document_id: &str) -> Result<(), String> {
    conn.execute(
        "DELETE FROM course_note_tags WHERE document_id = ?1",
        params![document_id],
    )
    .map(|_| ())
    .map_err(|err| storage_error("清理笔记标签关联失败", err))
}

// ---------------------------------------------------------------------------
// Tauri 命令
// ---------------------------------------------------------------------------

#[tauri::command]
pub(crate) fn course_search_notes(
    input: CourseSearchInput,
    database: State<'_, AppDatabase>,
) -> Result<CourseSearchOutput, String> {
    let conn = database
        .conn
        .lock()
        .map_err(|_| storage_error("数据库锁不可用", "标签与搜索不可用"))?;
    search_notes(&conn, input)
}

#[tauri::command]
pub(crate) fn course_load_note_tags(
    document_id: String,
    database: State<'_, AppDatabase>,
) -> Result<Vec<String>, String> {
    let conn = database
        .conn
        .lock()
        .map_err(|_| storage_error("数据库锁不可用", "标签读取不可用"))?;
    load_note_tags(&conn, &document_id)
}

#[tauri::command]
pub(crate) fn course_save_note_tags(
    input: CourseSaveTagsInput,
    database: State<'_, AppDatabase>,
) -> Result<Vec<String>, String> {
    let conn = database
        .conn
        .lock()
        .map_err(|_| storage_error("数据库锁不可用", "标签保存不可用"))?;
    save_note_tags(&conn, input)
}

#[tauri::command]
pub(crate) fn course_list_tags(
    database: State<'_, AppDatabase>,
) -> Result<Vec<CourseTagSummary>, String> {
    let conn = database
        .conn
        .lock()
        .map_err(|_| storage_error("数据库锁不可用", "标签列表不可用"))?;
    list_tags(&conn)
}

// ---------------------------------------------------------------------------
// 测试：内存 SQLite 上跑真实 SQL，验证业务效果而不是函数是否存在。
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().expect("打开内存数据库");
        conn.execute_batch(
            "PRAGMA foreign_keys = ON;
             CREATE TABLE documents (
               id TEXT PRIMARY KEY,
               title TEXT NOT NULL,
               body_md TEXT,
               content_type TEXT NOT NULL DEFAULT 'note',
               updated_at INTEGER NOT NULL DEFAULT 0
             );",
        )
        .expect("创建 documents 表");
        ensure_schema(&conn).expect("创建标签表");
        conn
    }

    fn insert_note(
        conn: &Connection,
        id: &str,
        title: &str,
        body: &str,
        content_type: &str,
        updated_at: i64,
    ) {
        conn.execute(
            "INSERT INTO documents (id, title, body_md, content_type, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, title, body, content_type, updated_at],
        )
        .expect("插入笔记");
    }

    fn search(conn: &Connection, query: &str) -> Result<CourseSearchOutput, String> {
        search_notes(
            conn,
            CourseSearchInput {
                query: query.to_string(),
                tag: None,
                limit: None,
            },
        )
    }

    fn search_with(
        conn: &Connection,
        query: &str,
        tag: Option<&str>,
        limit: Option<i64>,
    ) -> Result<CourseSearchOutput, String> {
        search_notes(
            conn,
            CourseSearchInput {
                query: query.to_string(),
                tag: tag.map(ToString::to_string),
                limit,
            },
        )
    }

    fn save(conn: &Connection, id: &str, tags: &[&str]) -> Result<Vec<String>, String> {
        save_note_tags(
            conn,
            CourseSaveTagsInput {
                document_id: id.to_string(),
                tags: tags.iter().map(|tag| tag.to_string()).collect(),
            },
        )
    }

    #[test]
    fn chinese_keyword_only_in_body_is_found() {
        let conn = test_conn();
        insert_note(
            &conn,
            "note-1",
            "课程笔记",
            "这是一段包含软件工程关键词的实际正文摘要。",
            "note",
            1,
        );

        let out = search(&conn, "软件工程").expect("搜索成功");
        assert_eq!(out.total, 1);
        assert!(!out.truncated);
        assert_eq!(out.items.len(), 1);
        assert_eq!(out.items[0].document_id, "note-1");
        assert_eq!(out.items[0].title, "课程笔记");
        assert_eq!(out.items[0].content_type, "note");
        assert!(out.items[0].snippet.contains("软件工程"));
        assert!(out.items[0].tags.is_empty());
    }

    #[test]
    fn title_hit_is_found_and_snippet_falls_back_to_body_start() {
        let conn = test_conn();
        insert_note(&conn, "note-1", "软件工程导论", "本章介绍需求分析。", "note", 1);

        let out = search(&conn, "软件工程").expect("搜索成功");
        assert_eq!(out.total, 1);
        assert_eq!(out.items[0].snippet, "本章介绍需求分析。");
    }

    #[test]
    fn no_match_is_an_empty_result_not_an_error() {
        let conn = test_conn();
        insert_note(&conn, "note-1", "课程笔记", "编程语言入门", "note", 1);

        let out = search(&conn, "不存在的词").expect("空结果不是错误");
        assert_eq!(out.total, 0);
        assert!(out.items.is_empty());
        assert!(!out.truncated);
    }

    #[test]
    fn empty_query_lists_notes_newest_first_and_breaks_ties_by_id() {
        let conn = test_conn();
        insert_note(&conn, "note-a", "A", "a", "note", 10);
        insert_note(&conn, "note-b", "B", "b", "note", 20);
        insert_note(&conn, "note-c", "C", "c", "text", 20);
        insert_note(&conn, "pdf-1", "PDF", "pdf", "pdf", 30);

        let out = search(&conn, "").expect("列出笔记");
        let ids: Vec<&str> = out.items.iter().map(|item| item.document_id.as_str()).collect();
        // updated_at DESC，同值按 id ASC；pdf 不在笔记范围内。
        assert_eq!(ids, vec!["note-b", "note-c", "note-a"]);
        assert_eq!(out.total, 3);
    }

    #[test]
    fn special_characters_match_literally() {
        let conn = test_conn();
        insert_note(&conn, "note-1", "覆盖率", "本次测试覆盖率 100% 达标", "note", 1);
        insert_note(&conn, "note-2", "变量名", "a_b 是合法标识符", "note", 2);
        insert_note(&conn, "note-3", "引号", "这里出现 引号\"字符 与 '单引号'", "note", 3);

        assert_eq!(search(&conn, "100%").expect("搜到").total, 1);
        assert_eq!(search(&conn, "a_b").expect("搜到").total, 1);
        // LIKE 通配符不应生效：% 与 _ 都是普通字符。
        assert_eq!(search(&conn, "a%b").expect("搜不到").total, 0);
        assert_eq!(search(&conn, "%").expect("只有一条正文含 %").total, 1);
        assert_eq!(search(&conn, "_").expect("只有一条正文含 _").total, 1);
        assert_eq!(search(&conn, "引号\"字符").expect("搜到").total, 1);
        assert_eq!(search(&conn, "'单引号'").expect("搜到").total, 1);
    }

    #[test]
    fn ascii_letters_ignore_case_but_cjk_stays_exact() {
        let conn = test_conn();
        insert_note(&conn, "note-1", "Markdown 语法", "Vue 组件与 CodeMirror 编辑器", "note", 1);

        assert_eq!(search(&conn, "markdown").expect("搜到").total, 1);
        assert_eq!(search(&conn, "MARKDOWN").expect("搜到").total, 1);
        assert_eq!(search(&conn, "codemirror").expect("搜到").total, 1);
        assert_eq!(search(&conn, "组件").expect("搜到").total, 1);
        assert_eq!(search(&conn, "编辑器 ").expect("首尾空白被裁掉").total, 1);
    }

    #[test]
    fn limit_total_and_truncated_are_reported() {
        let conn = test_conn();
        insert_note(&conn, "note-1", "一", "内容一", "note", 1);
        insert_note(&conn, "note-2", "二", "内容二", "note", 2);
        insert_note(&conn, "note-3", "三", "内容三", "note", 3);

        let out = search_with(&conn, "", None, Some(2)).expect("搜索成功");
        assert_eq!(out.items.len(), 2);
        assert_eq!(out.items[0].document_id, "note-3");
        assert_eq!(out.items[1].document_id, "note-2");
        assert_eq!(out.total, 3);
        assert!(out.truncated);

        let full = search_with(&conn, "", None, Some(3)).expect("搜索成功");
        assert_eq!(full.items.len(), 3);
        assert!(!full.truncated);
    }

    #[test]
    fn limit_outside_one_to_hundred_is_rejected() {
        let conn = test_conn();
        for bad in [0_i64, -1, 101] {
            let err = search_with(&conn, "", None, Some(bad)).expect_err("应当拒绝");
            assert!(err.starts_with(ERR_INVALID_INPUT), "实际错误：{err}");
        }
    }

    #[test]
    fn query_longer_than_the_cap_is_rejected() {
        let conn = test_conn();
        let too_long = "字".repeat(MAX_QUERY_CHARS + 1);
        let err = search(&conn, &too_long).expect_err("应当拒绝");
        assert!(err.starts_with(ERR_INVALID_INPUT));
        // 正好 100 个字符是允许的。
        let ok = "字".repeat(MAX_QUERY_CHARS);
        search(&conn, &ok).expect("100 字符应当允许");
    }

    #[test]
    fn tags_save_load_filter_reuse_and_clear() {
        let conn = test_conn();
        insert_note(&conn, "note-1", "笔记一", "包含 关键词 的正文", "note", 1);
        insert_note(&conn, "note-2", "笔记二", "另一篇正文", "note", 2);

        // 保存：返回规范化数组；读取按名称排序。
        let saved = save(&conn, "note-1", &["课程", "复习"]).expect("保存成功");
        assert_eq!(saved, vec!["课程", "复习"]);
        assert_eq!(load_note_tags(&conn, "note-1").expect("读取"), vec!["复习", "课程"]);

        // 复用：另一篇笔记用同名标签，标签记录只有一条，noteCount 累加。
        save(&conn, "note-2", &["课程"]).expect("保存成功");
        let all = list_tags(&conn).expect("标签列表");
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].name, "复习");
        assert_eq!(all[0].note_count, 1);
        assert_eq!(all[1].name, "课程");
        assert_eq!(all[1].note_count, 2);

        // 筛选：标签与关键词同时满足。
        let tagged = search_with(&conn, "", Some("课程"), None).expect("按标签筛选");
        assert_eq!(tagged.total, 2);
        assert_eq!(tagged.items[0].document_id, "note-2");
        assert_eq!(tagged.items[0].tags, vec!["课程"]);

        let both = search_with(&conn, "关键词", Some("课程"), None).expect("标签+关键词");
        assert_eq!(both.total, 1);
        assert_eq!(both.items[0].document_id, "note-1");

        let unknown_tag = search_with(&conn, "", Some("不存在的标签"), None).expect("正常空结果");
        assert_eq!(unknown_tag.total, 0);

        // 清空：空数组表示清掉该笔记的标签，其他笔记不受影响。
        assert!(save(&conn, "note-1", &[]).expect("清空成功").is_empty());
        assert!(load_note_tags(&conn, "note-1").expect("读取").is_empty());
        let after = list_tags(&conn).expect("标签列表");
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].name, "课程");
        assert_eq!(after[0].note_count, 1);
    }

    #[test]
    fn same_tag_with_different_ascii_case_is_reused() {
        let conn = test_conn();
        insert_note(&conn, "note-1", "一", "正文", "note", 1);
        insert_note(&conn, "note-2", "二", "正文", "note", 2);

        assert_eq!(save(&conn, "note-1", &["Course"]).expect("保存"), vec!["Course"]);
        // 第二次用小写保存：复用 'Course' 这条记录，返回实际保存的写法。
        assert_eq!(save(&conn, "note-2", &["course"]).expect("保存"), vec!["Course"]);
        let all = list_tags(&conn).expect("标签列表");
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].name, "Course");
        assert_eq!(all[0].note_count, 2);
    }

    #[test]
    fn duplicate_tags_in_one_save_are_deduplicated() {
        let conn = test_conn();
        insert_note(&conn, "note-1", "一", "正文", "note", 1);
        let saved = save(&conn, "note-1", &["课程", "课程", "COURSE", "course"]).expect("保存");
        assert_eq!(saved, vec!["课程", "COURSE"]);
    }

    #[test]
    fn unknown_document_id_is_reported() {
        let conn = test_conn();
        let load_err = load_note_tags(&conn, "missing").expect_err("应当报错");
        assert!(load_err.starts_with(ERR_NOTE_NOT_FOUND), "实际：{load_err}");
        let save_err = save(&conn, "missing", &["课程"]).expect_err("应当报错");
        assert!(save_err.starts_with(ERR_NOTE_NOT_FOUND), "实际：{save_err}");
    }

    #[test]
    fn unsupported_content_type_is_reported() {
        let conn = test_conn();
        insert_note(&conn, "pdf-1", "论文", "正文", "pdf", 1);
        insert_note(&conn, "web-1", "网页", "正文", "web", 2);

        for id in ["pdf-1", "web-1"] {
            let err = load_note_tags(&conn, id).expect_err("应当报错");
            assert!(err.starts_with(ERR_NOTE_NOT_EDITABLE), "实际：{err}");
            let err = save(&conn, id, &["课程"]).expect_err("应当报错");
            assert!(err.starts_with(ERR_NOTE_NOT_EDITABLE), "实际：{err}");
        }
    }

    #[test]
    fn invalid_tags_are_rejected_and_leave_stored_tags_untouched() {
        let conn = test_conn();
        insert_note(&conn, "note-1", "一", "正文", "note", 1);
        save(&conn, "note-1", &["课程"]).expect("先保存一个合法标签");

        let too_long = "字".repeat(MAX_TAG_CHARS + 1);
        let too_many: Vec<String> = (1..=MAX_TAGS_PER_NOTE + 1)
            .map(|index| format!("tag{index}"))
            .collect();
        let cases: Vec<Vec<String>> = vec![
            vec![String::new()],
            vec!["   ".to_string()],
            vec![too_long],
            vec!["a\nb".to_string()],
            too_many,
        ];
        for case in cases {
            let err = save_note_tags(
                &conn,
                CourseSaveTagsInput {
                    document_id: "note-1".to_string(),
                    tags: case.clone(),
                },
            )
            .expect_err("应当拒绝");
            assert!(err.starts_with(ERR_INVALID_INPUT), "案例 {case:?} 实际：{err}");
        }

        // 被拒绝的写入不能留下半套标签。
        assert_eq!(load_note_tags(&conn, "note-1").expect("读取"), vec!["课程"]);
        let all = list_tags(&conn).expect("标签列表");
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].name, "课程");
    }

    #[test]
    fn snippet_is_capped_and_centered_on_the_hit() {
        let conn = test_conn();
        let body = format!("{}关键词{}", "前".repeat(60), "后".repeat(60));
        insert_note(&conn, "note-1", "长文", &body, "note", 1);

        let out = search(&conn, "关键词").expect("搜索成功");
        let snippet = &out.items[0].snippet;
        assert!(snippet.contains("关键词"), "摘要丢了命中：{snippet}");
        assert!(
            snippet.chars().count() <= MAX_SNIPPET_CHARS,
            "摘要超长：{} 字符",
            snippet.chars().count()
        );
        // 命中点离正文开头很远，摘要应当从中间截取。
        assert!(snippet.starts_with('…'), "摘要未截取前文：{snippet}");
    }

    #[test]
    fn blank_body_yields_an_empty_snippet() {
        let conn = test_conn();
        insert_note(&conn, "note-1", "空笔记", "", "note", 1);
        let out = search(&conn, "").expect("搜索成功");
        assert_eq!(out.items[0].snippet, "");
    }

    #[test]
    fn schema_initialisation_is_idempotent() {
        let conn = test_conn();
        ensure_schema(&conn).expect("第二次初始化");
        ensure_schema(&conn).expect("第三次初始化");
        insert_note(&conn, "note-1", "一", "正文", "note", 1);
        assert_eq!(save(&conn, "note-1", &["课程"]).expect("保存"), vec!["课程"]);
    }

    #[test]
    fn deleting_a_note_clears_only_its_own_tag_links() {
        let conn = test_conn();
        insert_note(&conn, "note-1", "一", "正文", "note", 1);
        insert_note(&conn, "note-2", "二", "正文", "note", 2);
        save(&conn, "note-1", &["课程", "复习"]).expect("保存");
        save(&conn, "note-2", &["课程"]).expect("保存");

        conn.execute("DELETE FROM documents WHERE id = 'note-1'", [])
            .expect("删除笔记");

        let links: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM course_note_tags WHERE document_id = 'note-1'",
                [],
                |row| row.get(0),
            )
            .expect("统计关联");
        assert_eq!(links, 0, "删除笔记后不应留下关联");

        let all = list_tags(&conn).expect("标签列表");
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].name, "课程");
        assert_eq!(all[0].note_count, 1, "其他笔记的标签必须保留");
    }

    #[test]
    fn explicit_cleanup_removes_the_association() {
        let conn = test_conn();
        insert_note(&conn, "note-1", "一", "正文", "note", 1);
        save(&conn, "note-1", &["课程"]).expect("保存");
        delete_associations(&conn, "note-1").expect("显式清理");
        assert!(list_tags(&conn).expect("标签列表").is_empty());
    }
}
