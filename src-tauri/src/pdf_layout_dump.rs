use std::collections::HashMap;

use rusqlite::{params, Connection, OptionalExtension};
use tauri::State;

use crate::{
    pdf_index::{debug_pdfium_layout_from_lines, PdfLayoutDebugLine},
    AppDatabase, CURRENT_INDEX_VERSION,
};

struct LayoutDumpLine {
    value: serde_json::Value,
    debug_line: Option<PdfLayoutDebugLine>,
}

#[tauri::command]
pub(crate) fn dump_pdf_layout(
    document_id: String,
    page_no: u32,
    database: State<'_, AppDatabase>,
) -> Result<serde_json::Value, String> {
    let document_id = document_id.trim();
    if document_id.is_empty() {
        return Err("No document id provided for layout dump".to_string());
    }
    if page_no == 0 {
        return Err("Page number must be greater than zero".to_string());
    }

    let conn = database
        .conn
        .lock()
        .map_err(|_| "SQLite lock was poisoned".to_string())?;
    let page = conn
        .query_row(
            "SELECT page_no, width, height, text
             FROM document_pages
             WHERE document_id = ?1 AND page_no = ?2",
            params![document_id, page_no],
            |row| {
                Ok((
                    row.get::<_, u32>(0)?,
                    row.get::<_, f64>(1)?,
                    row.get::<_, f64>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .optional()
        .map_err(|err| format!("Failed to read indexed page for layout dump: {err}"))?
        .ok_or_else(|| format!("No indexed page {page_no} found for document {document_id}"))?;
    let document = conn
        .query_row(
            "SELECT index_status, index_version FROM documents WHERE id = ?1",
            params![document_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()
        .map_err(|err| format!("Failed to read document index metadata for layout dump: {err}"))?;

    let layout_lines = read_layout_dump_layout_lines(&conn, document_id, page_no)?;
    let lines = read_layout_dump_lines(&conn, document_id, page_no, page.1, page.2)?;
    let units = read_layout_dump_units(&conn, document_id, page_no)?;
    let blocks = read_layout_dump_blocks(&conn, document_id, page_no)?;
    let stored_regions = read_layout_dump_regions(&conn, document_id, page_no)?;
    let layout_debug = debug_pdfium_layout_from_lines(
        lines
            .iter()
            .filter_map(|line| line.debug_line.clone())
            .collect(),
        page.1,
        page.2,
    );
    let mut warnings = layout_dump_warnings(&blocks, page.1, page.2);
    warnings.extend(layout_debug.warnings.iter().map(|warning| {
        serde_json::json!({
            "kind": warning.kind,
            "message": warning.message,
        })
    }));
    let line_values = lines.into_iter().map(|line| line.value).collect::<Vec<_>>();
    let (regions, regions_source) = if stored_regions.is_empty() {
        (serde_json::json!(layout_debug.regions), "computed")
    } else {
        (serde_json::json!(stored_regions), "stored")
    };

    Ok(serde_json::json!({
        "documentId": document_id,
        "page": page.0,
        "currentIndexVersion": CURRENT_INDEX_VERSION,
        "documentIndexStatus": document.as_ref().map(|value| value.0.as_str()),
        "documentIndexVersion": document.as_ref().map(|value| value.1),
        "width": page.1,
        "height": page.2,
        "lineCount": line_values.len(),
        "unitCount": units.len(),
        "blockCount": blocks.len(),
        "layoutConfidence": layout_debug.confidence,
        "twoColumnDetected": layout_debug.two_column_detected,
        "regionsSource": regions_source,
        "pageTextPreview": page.3.chars().take(600).collect::<String>(),
        "units": units,
        "layoutLines": layout_lines,
        "lines": line_values,
        "regions": regions,
        "blocks": blocks,
        "warnings": warnings,
    }))
}

fn read_layout_dump_units(
    conn: &Connection,
    document_id: &str,
    page_no: u32,
) -> Result<Vec<serde_json::Value>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, source_order, text, bbox_json, font_size, font_name, font_flags
             FROM document_text_units
             WHERE document_id = ?1 AND page_no = ?2
             ORDER BY source_order",
        )
        .map_err(|err| format!("Failed to prepare layout text unit dump: {err}"))?;
    let rows = stmt
        .query_map(params![document_id, page_no], |row| {
            let bbox_json: String = row.get(3)?;
            let bbox: serde_json::Value =
                serde_json::from_str(&bbox_json).unwrap_or_else(|_| serde_json::json!([]));
            Ok(serde_json::json!({
                "id": row.get::<_, String>(0)?,
                "sourceOrder": row.get::<_, u32>(1)?,
                "text": row.get::<_, String>(2)?,
                "bbox": bbox,
                "fontSize": row.get::<_, f64>(4)?,
                "fontName": row.get::<_, String>(5)?,
                "fontFlags": row.get::<_, i64>(6)?,
            }))
        })
        .map_err(|err| format!("Failed to read layout text unit dump: {err}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|err| format!("Failed to collect layout text unit dump: {err}"))
}

fn read_layout_dump_layout_lines(
    conn: &Connection,
    document_id: &str,
    page_no: u32,
) -> Result<Vec<serde_json::Value>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, line_no, text, bbox_json, source_order_json, region_index, region_id, role_hint, baseline
             FROM document_layout_lines
             WHERE document_id = ?1 AND page_no = ?2
             ORDER BY line_no",
        )
        .map_err(|err| format!("Failed to prepare raw layout line dump: {err}"))?;
    let rows = stmt
        .query_map(params![document_id, page_no], |row| {
            let bbox_json: String = row.get(3)?;
            let bbox_list: serde_json::Value =
                serde_json::from_str(&bbox_json).unwrap_or_else(|_| serde_json::json!([]));
            let source_order_json: String = row.get(4)?;
            let source_order_list: serde_json::Value =
                serde_json::from_str(&source_order_json).unwrap_or_else(|_| serde_json::json!([]));
            Ok(serde_json::json!({
                "id": row.get::<_, String>(0)?,
                "lineNo": row.get::<_, u32>(1)?,
                "text": row.get::<_, String>(2)?,
                "bboxList": bbox_list,
                "sourceOrderList": source_order_list,
                "regionIndex": row.get::<_, u32>(5)?,
                "regionId": row.get::<_, String>(6)?,
                "roleHint": row.get::<_, String>(7)?,
                "baseline": row.get::<_, f64>(8)?,
            }))
        })
        .map_err(|err| format!("Failed to read raw layout line dump: {err}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|err| format!("Failed to collect raw layout line dump: {err}"))
}

fn read_layout_dump_lines(
    conn: &Connection,
    document_id: &str,
    page_no: u32,
    page_width: f64,
    page_height: f64,
) -> Result<Vec<LayoutDumpLine>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, line_no, block_id, block_index, text, bbox_json, source_order_json, region_index, region_id
             FROM document_lines
             WHERE document_id = ?1 AND page_no = ?2
             ORDER BY line_no",
        )
        .map_err(|err| format!("Failed to prepare layout line dump: {err}"))?;
    let rows = stmt
        .query_map(params![document_id, page_no], |row| {
            let bbox_json: String = row.get(5)?;
            let bbox_list: serde_json::Value =
                serde_json::from_str(&bbox_json).unwrap_or_else(|_| serde_json::json!([]));
            let source_order_json: String = row.get(6)?;
            let source_order_list: serde_json::Value =
                serde_json::from_str(&source_order_json).unwrap_or_else(|_| serde_json::json!([]));
            let line_no = row.get::<_, u32>(1)?;
            let block_index = row.get::<_, u32>(3)?;
            let text = row.get::<_, String>(4)?;
            let debug_line = debug_line_from_bbox(
                line_no,
                block_index,
                text.clone(),
                &bbox_list,
                page_width,
                page_height,
            );
            let value = serde_json::json!({
                "id": row.get::<_, String>(0)?,
                "lineNo": line_no,
                "blockId": row.get::<_, String>(2)?,
                "blockIndex": block_index,
                "text": text,
                "bboxList": bbox_list,
                "sourceOrderList": source_order_list,
                "regionIndex": row.get::<_, u32>(7)?,
                "regionId": row.get::<_, String>(8)?,
            });
            Ok(LayoutDumpLine { value, debug_line })
        })
        .map_err(|err| format!("Failed to read layout dump lines: {err}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|err| format!("Failed to collect layout dump lines: {err}"))
}

fn debug_line_from_bbox(
    line_no: u32,
    block_index: u32,
    text: String,
    bbox_list: &serde_json::Value,
    page_width: f64,
    page_height: f64,
) -> Option<PdfLayoutDebugLine> {
    let bbox = union_normalized_bbox(bbox_list)?;
    Some(PdfLayoutDebugLine {
        line_no,
        block_index,
        text,
        x1: bbox[0] * page_width,
        y1: bbox[1] * page_height,
        x2: bbox[2] * page_width,
        y2: bbox[3] * page_height,
    })
}

fn union_normalized_bbox(bbox_list: &serde_json::Value) -> Option<[f64; 4]> {
    let items = bbox_list.as_array()?;
    let mut x1 = f64::INFINITY;
    let mut y1 = f64::INFINITY;
    let mut x2 = f64::NEG_INFINITY;
    let mut y2 = f64::NEG_INFINITY;
    let mut found = false;
    for item in items {
        let Some(bbox) = item.as_array() else {
            continue;
        };
        if bbox.len() < 4 {
            continue;
        }
        let Some(left) = bbox[0].as_f64() else {
            continue;
        };
        let Some(top) = bbox[1].as_f64() else {
            continue;
        };
        let Some(right) = bbox[2].as_f64() else {
            continue;
        };
        let Some(bottom) = bbox[3].as_f64() else {
            continue;
        };
        if ![left, top, right, bottom].into_iter().all(f64::is_finite) {
            continue;
        }
        x1 = x1.min(left);
        y1 = y1.min(top);
        x2 = x2.max(right);
        y2 = y2.max(bottom);
        found = true;
    }
    found.then_some([x1, y1, x2, y2])
}

fn read_layout_dump_blocks(
    conn: &Connection,
    document_id: &str,
    page_no: u32,
) -> Result<Vec<serde_json::Value>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, block_index, text, bbox_json, block_role, region_index, region_id
             FROM document_blocks
             WHERE document_id = ?1 AND page_no = ?2
             ORDER BY block_index",
        )
        .map_err(|err| format!("Failed to prepare layout block dump: {err}"))?;
    let rows = stmt
        .query_map(params![document_id, page_no], |row| {
            let bbox_json: String = row.get(3)?;
            let bbox_list: serde_json::Value =
                serde_json::from_str(&bbox_json).unwrap_or_else(|_| serde_json::json!([]));
            Ok(serde_json::json!({
                "id": row.get::<_, String>(0)?,
                "blockIndex": row.get::<_, u32>(1)?,
                "text": row.get::<_, String>(2)?,
                "bboxList": bbox_list,
                "role": row.get::<_, String>(4)?,
                "regionIndex": row.get::<_, u32>(5)?,
                "regionId": row.get::<_, String>(6)?,
            }))
        })
        .map_err(|err| format!("Failed to read layout dump blocks: {err}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|err| format!("Failed to collect layout dump blocks: {err}"))
}

fn read_layout_dump_regions(
    conn: &Connection,
    document_id: &str,
    page_no: u32,
) -> Result<Vec<serde_json::Value>, String> {
    let line_block_indexes = read_line_block_indexes(conn, document_id, page_no)?;
    let mut stmt = conn
        .prepare(
            "SELECT id, region_index, kind, bbox_json, line_numbers_json, confidence
             FROM document_layout_regions
             WHERE document_id = ?1 AND page_no = ?2
             ORDER BY region_index",
        )
        .map_err(|err| format!("Failed to prepare layout region dump: {err}"))?;
    let rows = stmt
        .query_map(params![document_id, page_no], |row| {
            let bbox_json: String = row.get(3)?;
            let bbox: serde_json::Value =
                serde_json::from_str(&bbox_json).unwrap_or_else(|_| serde_json::json!([]));
            let line_numbers_json: String = row.get(4)?;
            let line_numbers =
                serde_json::from_str::<Vec<u32>>(&line_numbers_json).unwrap_or_default();
            let mut block_indexes = line_numbers
                .iter()
                .filter_map(|line_no| line_block_indexes.get(line_no).copied())
                .filter(|block_index| *block_index > 0)
                .collect::<Vec<_>>();
            block_indexes.sort_unstable();
            block_indexes.dedup();
            Ok(serde_json::json!({
                "id": row.get::<_, String>(0)?,
                "index": row.get::<_, u32>(1)?,
                "kind": row.get::<_, String>(2)?,
                "lineCount": line_numbers.len(),
                "lineNumbers": line_numbers,
                "blockIndexes": block_indexes,
                "bbox": bbox,
                "confidence": row.get::<_, f64>(5)?,
            }))
        })
        .map_err(|err| format!("Failed to read layout dump regions: {err}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|err| format!("Failed to collect layout dump regions: {err}"))
}

fn read_line_block_indexes(
    conn: &Connection,
    document_id: &str,
    page_no: u32,
) -> Result<HashMap<u32, u32>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT line_no, block_index
             FROM document_lines
             WHERE document_id = ?1 AND page_no = ?2",
        )
        .map_err(|err| format!("Failed to prepare layout line block lookup: {err}"))?;
    let rows = stmt
        .query_map(params![document_id, page_no], |row| {
            Ok((row.get::<_, u32>(0)?, row.get::<_, u32>(1)?))
        })
        .map_err(|err| format!("Failed to read layout line block lookup: {err}"))?;
    rows.collect::<Result<HashMap<_, _>, _>>()
        .map_err(|err| format!("Failed to collect layout line block lookup: {err}"))
}

fn layout_dump_warnings(
    blocks: &[serde_json::Value],
    page_width: f64,
    page_height: f64,
) -> Vec<serde_json::Value> {
    let mut warnings = Vec::new();
    if !page_width.is_finite()
        || !page_height.is_finite()
        || page_width <= 0.0
        || page_height <= 0.0
    {
        warnings.push(serde_json::json!({
            "kind": "invalid_page_size",
            "message": "Indexed page size is invalid; bbox interpretation may be unreliable."
        }));
    }
    for block in blocks {
        let Some(text) = block.get("text").and_then(|value| value.as_str()) else {
            continue;
        };
        let Some(role) = block.get("role").and_then(|value| value.as_str()) else {
            continue;
        };
        if role == "body" && looks_like_layout_dump_code(text) {
            warnings.push(serde_json::json!({
                "kind": "possible_code_role_miss",
                "blockIndex": block.get("blockIndex").cloned().unwrap_or(serde_json::Value::Null),
                "message": "This body block looks code-like but was not indexed as code."
            }));
        }
        if role == "body" && looks_like_layout_dump_caption(text) {
            warnings.push(serde_json::json!({
                "kind": "possible_caption_role_miss",
                "blockIndex": block.get("blockIndex").cloned().unwrap_or(serde_json::Value::Null),
                "message": "This body block looks caption-like but was not indexed as caption."
            }));
        }
    }
    warnings
}

fn looks_like_layout_dump_code(text: &str) -> bool {
    let lower = text.trim().to_lowercase();
    lower.starts_with("def ")
        || lower.starts_with("class ")
        || lower.starts_with("return ")
        || lower.contains("raw_output")
        || lower.contains("file_path")
}

fn looks_like_layout_dump_caption(text: &str) -> bool {
    let lower = text.trim().to_lowercase();
    lower.starts_with("figure ")
        || lower.starts_with("fig. ")
        || lower.starts_with("table ")
        || lower.starts_with("algorithm ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn union_normalized_bbox_merges_multiple_rects() {
        let bbox_list = serde_json::json!([[0.10, 0.20, 0.30, 0.40], [0.12, 0.18, 0.45, 0.50]]);

        assert_eq!(
            union_normalized_bbox(&bbox_list),
            Some([0.10, 0.18, 0.45, 0.50])
        );
    }

    #[test]
    fn debug_line_from_bbox_converts_to_page_coordinates() {
        let bbox_list = serde_json::json!([[0.10, 0.20, 0.30, 0.40]]);
        let line = debug_line_from_bbox(7, 3, "left column".to_string(), &bbox_list, 600.0, 800.0)
            .expect("debug line");

        assert_eq!(line.line_no, 7);
        assert_eq!(line.block_index, 3);
        assert_eq!(line.x1, 60.0);
        assert_eq!(line.y1, 160.0);
        assert_eq!(line.x2, 180.0);
        assert_eq!(line.y2, 320.0);
    }

    #[test]
    fn read_layout_dump_units_orders_by_pdf_source_order() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        conn.execute_batch(
            "CREATE TABLE document_text_units (
              id TEXT PRIMARY KEY,
              document_id TEXT NOT NULL,
              page_no INTEGER NOT NULL,
              source_order INTEGER NOT NULL,
              text TEXT NOT NULL,
              bbox_json TEXT NOT NULL,
              font_size REAL NOT NULL DEFAULT 0,
              font_name TEXT NOT NULL DEFAULT '',
              font_flags INTEGER NOT NULL DEFAULT 0
            );
            INSERT INTO document_text_units
              (id, document_id, page_no, source_order, text, bbox_json, font_size, font_name, font_flags)
            VALUES
              ('u2', 'doc', 1, 2, 'second', '[0.2,0.1,0.3,0.2]', 0, '', 0),
              ('u1', 'doc', 1, 1, 'first', '[0.1,0.1,0.2,0.2]', 0, '', 0);",
        )
        .expect("schema");

        let units = read_layout_dump_units(&conn, "doc", 1).expect("units");

        assert_eq!(units.len(), 2);
        assert_eq!(units[0]["sourceOrder"], serde_json::json!(1));
        assert_eq!(units[0]["text"], serde_json::json!("first"));
        assert_eq!(units[1]["sourceOrder"], serde_json::json!(2));
    }

    #[test]
    fn read_layout_dump_lines_exposes_region_reference() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        conn.execute_batch(
            "CREATE TABLE document_lines (
              id TEXT PRIMARY KEY,
              document_id TEXT NOT NULL,
              page_no INTEGER NOT NULL,
              line_no INTEGER NOT NULL,
              block_id TEXT NOT NULL DEFAULT '',
              block_index INTEGER NOT NULL DEFAULT 0,
              text TEXT NOT NULL,
              bbox_json TEXT NOT NULL,
              source_order_json TEXT NOT NULL DEFAULT '[]',
              region_index INTEGER NOT NULL DEFAULT 0,
              region_id TEXT NOT NULL DEFAULT ''
            );
            INSERT INTO document_lines
              (id, document_id, page_no, line_no, block_id, block_index, text, bbox_json, source_order_json, region_index, region_id)
            VALUES
              ('l1', 'doc', 1, 1, 'b1', 1, 'line text', '[[0.1,0.2,0.3,0.4]]', '[7]', 5, 'doc-p1-r5');",
        )
        .expect("schema");

        let lines = read_layout_dump_lines(&conn, "doc", 1, 600.0, 800.0).expect("lines");

        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].value["regionIndex"], serde_json::json!(5));
        assert_eq!(lines[0].value["regionId"], serde_json::json!("doc-p1-r5"));
        assert_eq!(lines[0].value["sourceOrderList"], serde_json::json!([7]));
    }

    #[test]
    fn read_layout_dump_layout_lines_exposes_raw_line_layer() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        conn.execute_batch(
            "CREATE TABLE document_layout_lines (
              id TEXT PRIMARY KEY,
              document_id TEXT NOT NULL,
              page_no INTEGER NOT NULL,
              line_no INTEGER NOT NULL,
              text TEXT NOT NULL,
              bbox_json TEXT NOT NULL,
              source_order_json TEXT NOT NULL DEFAULT '[]',
              region_index INTEGER NOT NULL DEFAULT 0,
              region_id TEXT NOT NULL DEFAULT '',
              role_hint TEXT NOT NULL DEFAULT '',
              baseline REAL NOT NULL DEFAULT 0
            );
            INSERT INTO document_layout_lines
              (id, document_id, page_no, line_no, text, bbox_json, source_order_json, region_index, region_id, role_hint, baseline)
            VALUES
              ('rl1', 'doc', 1, 1, 'raw line', '[[0.1,0.2,0.3,0.4]]', '[7,8]', 5, 'doc-p1-r5', 'body', 123.4);",
        )
        .expect("schema");

        let lines = read_layout_dump_layout_lines(&conn, "doc", 1).expect("raw lines");

        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0]["lineNo"], serde_json::json!(1));
        assert_eq!(lines[0]["sourceOrderList"], serde_json::json!([7, 8]));
        assert_eq!(lines[0]["regionIndex"], serde_json::json!(5));
        assert_eq!(lines[0]["regionId"], serde_json::json!("doc-p1-r5"));
        assert_eq!(lines[0]["roleHint"], serde_json::json!("body"));
        assert_eq!(lines[0]["baseline"], serde_json::json!(123.4));
    }

    #[test]
    fn read_layout_dump_regions_adds_line_and_block_references() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        conn.execute_batch(
            "CREATE TABLE document_lines (
              id TEXT PRIMARY KEY,
              document_id TEXT NOT NULL,
              page_no INTEGER NOT NULL,
              line_no INTEGER NOT NULL,
              block_index INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE document_layout_regions (
              id TEXT PRIMARY KEY,
              document_id TEXT NOT NULL,
              page_no INTEGER NOT NULL,
              region_index INTEGER NOT NULL,
              kind TEXT NOT NULL,
              bbox_json TEXT NOT NULL,
              line_numbers_json TEXT NOT NULL DEFAULT '[]',
              confidence REAL NOT NULL DEFAULT 0
            );
            INSERT INTO document_lines
              (id, document_id, page_no, line_no, block_index)
            VALUES
              ('l1', 'doc', 1, 1, 2),
              ('l2', 'doc', 1, 2, 2),
              ('l3', 'doc', 1, 3, 3);
            INSERT INTO document_layout_regions
              (id, document_id, page_no, region_index, kind, bbox_json, line_numbers_json, confidence)
            VALUES
              ('r1', 'doc', 1, 1, 'body_column', '[0.1,0.2,0.3,0.4]', '[1,2]', 0.95);",
        )
        .expect("schema");

        let regions = read_layout_dump_regions(&conn, "doc", 1).expect("regions");

        assert_eq!(regions.len(), 1);
        assert_eq!(regions[0]["kind"], serde_json::json!("body_column"));
        assert_eq!(regions[0]["lineNumbers"], serde_json::json!([1, 2]));
        assert_eq!(regions[0]["blockIndexes"], serde_json::json!([2]));
    }

    #[test]
    fn read_layout_dump_blocks_exposes_region_reference() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        conn.execute_batch(
            "CREATE TABLE document_blocks (
              id TEXT PRIMARY KEY,
              document_id TEXT NOT NULL,
              page_no INTEGER NOT NULL,
              block_index INTEGER NOT NULL,
              text TEXT NOT NULL,
              bbox_json TEXT NOT NULL,
              block_role TEXT NOT NULL DEFAULT 'body',
              region_index INTEGER NOT NULL DEFAULT 0,
              region_id TEXT NOT NULL DEFAULT ''
            );
            INSERT INTO document_blocks
              (id, document_id, page_no, block_index, text, bbox_json, block_role, region_index, region_id)
            VALUES
              ('b1', 'doc', 1, 1, 'paragraph', '[[0.1,0.2,0.3,0.4]]', 'body', 4, 'doc-p1-r4');",
        )
        .expect("schema");

        let blocks = read_layout_dump_blocks(&conn, "doc", 1).expect("blocks");

        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0]["regionIndex"], serde_json::json!(4));
        assert_eq!(blocks[0]["regionId"], serde_json::json!("doc-p1-r4"));
    }
}
