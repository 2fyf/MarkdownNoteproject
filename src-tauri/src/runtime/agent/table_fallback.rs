//! Shared decision for "open_table evidence was sparse — fall back to open_pages".
//!
//! Both M3 (`turn_runner`) and M4 (`agent_judge`) loops need the same heuristic:
//! if `open_table` returned too few citations or noisy placeholder column
//! headers, we should also open the underlying page to pick up readable rows.
//! This module encapsulates the heuristic plus page selection.

use crate::runtime::agent::lexicon::requested_table_number;
use crate::runtime::rag::RagToolExecutionOutput;

const SPARSE_CITATION_THRESHOLD: usize = 6;
const SPARSE_QUOTE_CHAR_THRESHOLD: usize = 700;
const PLACEHOLDER_COLUMN_NEEDLES: &[&str] = &["column 1", "column 2", "column 3", "column 4"];

/// Decide whether `open_table`'s output is too sparse to answer on its own and
/// return the page number that should be opened next.
///
/// When `requested_table_no` is provided, the helper first tries to land on a
/// citation whose `section_title` parses back to the same table number, so
/// downstream evidence stays anchored to the user's referenced table.
pub fn sparse_open_table_page(
    output: &RagToolExecutionOutput,
    requested_table_no: Option<&str>,
) -> Option<u32> {
    if output.tool_call.tool != "open_table" {
        return None;
    }
    let page = pick_open_table_page(output, requested_table_no)?;
    if !output_looks_sparse(output) {
        return None;
    }
    Some(page)
}

fn pick_open_table_page(
    output: &RagToolExecutionOutput,
    requested_table_no: Option<&str>,
) -> Option<u32> {
    if let Some(target) = requested_table_no {
        if let Some(citation) = output.citations.iter().find(|citation| {
            citation.source == "open_table"
                && citation
                    .section_title
                    .as_deref()
                    .and_then(requested_table_number)
                    .as_deref()
                    == Some(target)
        }) {
            return Some(citation.page);
        }
    }
    output
        .citations
        .iter()
        .find(|citation| citation.source == "open_table")
        .map(|citation| citation.page)
        .or_else(|| output.citations.first().map(|citation| citation.page))
}

fn output_looks_sparse(output: &RagToolExecutionOutput) -> bool {
    let combined = output
        .citations
        .iter()
        .map(|citation| citation.quote.as_str())
        .collect::<Vec<_>>()
        .join("\n")
        .to_lowercase();
    let sparse_citations = output.citations.len() < SPARSE_CITATION_THRESHOLD
        || combined.chars().count() < SPARSE_QUOTE_CHAR_THRESHOLD;
    let placeholder_columns = PLACEHOLDER_COLUMN_NEEDLES
        .iter()
        .any(|needle| combined.contains(needle));
    sparse_citations || placeholder_columns
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::rag::{Citation, RetrievalTraceToolCall};

    fn cite(source: &str, section_title: Option<&str>, page: u32, quote: &str) -> Citation {
        Citation {
            id: format!("c-{page}-{source}"),
            label: "test".to_string(),
            page,
            block_id: format!("b-{page}"),
            section_title: section_title.map(str::to_string),
            quote: quote.to_string(),
            bbox_list: serde_json::json!([]),
            document_id: "doc".to_string(),
            source: source.to_string(),
        }
    }

    fn output(citations: Vec<Citation>) -> RagToolExecutionOutput {
        let count = citations.len();
        RagToolExecutionOutput {
            tool_call: RetrievalTraceToolCall {
                tool: "open_table".to_string(),
                status: "ok".to_string(),
                input: serde_json::json!({}),
                result_count: count,
                error: None,
            },
            citations,
            trace_candidates: Vec::new(),
            tree_nodes: Vec::new(),
        }
    }

    #[test]
    fn non_open_table_tool_returns_none() {
        let mut out = output(vec![cite("open_pages", None, 3, "row text")]);
        out.tool_call.tool = "open_pages".to_string();
        assert_eq!(sparse_open_table_page(&out, None), None);
    }

    #[test]
    fn placeholder_columns_trigger_fallback() {
        let out = output(vec![cite(
            "open_table",
            Some("Table 3"),
            7,
            "column 1 column 2 column 3 column 4",
        )]);
        assert_eq!(sparse_open_table_page(&out, None), Some(7));
    }

    #[test]
    fn few_citations_trigger_fallback() {
        let out = output(vec![
            cite("open_table", Some("Table 3"), 5, "GLM 10.0"),
            cite("open_table", Some("Table 3"), 5, "GPT 11.0"),
        ]);
        assert_eq!(sparse_open_table_page(&out, None), Some(5));
    }

    #[test]
    fn dense_long_table_does_not_trigger_fallback() {
        let long_quote: String = "row data ".repeat(120);
        let citations: Vec<Citation> = (0..6)
            .map(|_| cite("open_table", Some("Table 3"), 4, &long_quote))
            .collect();
        let out = output(citations);
        assert_eq!(sparse_open_table_page(&out, None), None);
    }

    #[test]
    fn requested_table_number_picks_matching_page() {
        let out = output(vec![
            cite("open_table", Some("Table 2"), 4, "a"),
            cite("open_table", Some("Table 5"), 9, "column 1"),
        ]);
        assert_eq!(sparse_open_table_page(&out, Some("5")), Some(9));
        assert_eq!(sparse_open_table_page(&out, Some("2")), Some(4));
    }
}
