use std::collections::HashSet;

use crate::{runtime, AskDocumentClaim};

fn resolve_claim_labels(
    labels: Vec<String>,
    citations: &[runtime::rag::Citation],
) -> (Vec<String>, Vec<String>) {
    let mut citation_ids = Vec::new();
    let mut citation_labels = Vec::new();
    for label in labels {
        let normalized = normalize_citation_label(&label);
        if normalized.is_empty() {
            continue;
        }
        if let Some(citation) = citations
            .iter()
            .find(|citation| normalize_citation_label(&citation.label) == normalized)
        {
            citation_ids.push(citation.id.clone());
            citation_labels.push(citation.label.clone());
        }
    }
    (citation_ids, citation_labels)
}

fn normalize_citation_label(label: &str) -> String {
    label
        .trim()
        .trim_matches(|ch| matches!(ch, '[' | ']'))
        .to_string()
}

pub(crate) fn fallback_claims_from_answer(
    answer: &str,
    citations: &[runtime::rag::Citation],
) -> Vec<AskDocumentClaim> {
    split_claim_segments(answer)
        .into_iter()
        .filter_map(|segment| {
            let labels = extract_inline_citation_labels(&segment);
            if labels.is_empty() {
                return None;
            }
            let (citation_ids, citation_labels) = resolve_claim_labels(labels, citations);
            if citation_ids.is_empty() {
                return None;
            }
            Some(AskDocumentClaim {
                text: segment,
                citation_ids,
                citation_labels,
            })
        })
        .collect()
}

fn split_claim_segments(answer: &str) -> Vec<String> {
    let mut segments = Vec::new();
    for raw_line in answer.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        let mut start = 0usize;
        for (index, ch) in line.char_indices() {
            if !matches!(ch, '.' | '?' | '!' | ';' | '。' | '？' | '！' | '；') {
                continue;
            }
            let end = index + ch.len_utf8();
            let segment = line[start..end].trim();
            if !segment.is_empty() {
                segments.push(segment.to_string());
            }
            start = end;
        }
        let tail = line[start..].trim();
        if !tail.is_empty() {
            segments.push(tail.to_string());
        }
    }
    segments
}

pub(crate) fn strip_known_inline_citation_labels(
    answer: &str,
    citations: &[runtime::rag::Citation],
) -> String {
    let mut cleaned = answer.to_string();
    let mut variants = HashSet::new();
    for citation in citations {
        let label = citation.label.trim();
        if label.is_empty() {
            continue;
        }
        variants.insert(label.to_string());
        let normalized = normalize_citation_label(label);
        if !normalized.is_empty() {
            variants.insert(format!("[{normalized}]"));
        }
    }
    let mut variants = variants.into_iter().collect::<Vec<_>>();
    variants.sort_by_key(|value| std::cmp::Reverse(value.len()));
    for variant in variants {
        cleaned = cleaned.replace(&format!(" {variant}"), "");
        cleaned = cleaned.replace(&variant, "");
    }

    cleaned
        .lines()
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect::<Vec<_>>()
        .join("\n")
        .replace(" .", ".")
        .replace(" ,", ",")
        .replace(" ;", ";")
        .replace(" :", ":")
        .trim()
        .to_string()
}

fn extract_inline_citation_labels(value: &str) -> Vec<String> {
    let mut labels = Vec::new();
    let mut chars = value.char_indices().peekable();
    while let Some((start_index, ch)) = chars.next() {
        if ch != '[' {
            continue;
        }
        let mut end_index = None;
        while let Some((index, next_ch)) = chars.peek().copied() {
            if next_ch == ']' {
                end_index = Some(index);
                chars.next();
                break;
            }
            chars.next();
        }
        if let Some(end_index) = end_index {
            let raw = &value[start_index + 1..end_index];
            if raw.chars().all(|ch| ch.is_ascii_digit()) {
                labels.push(format!("[{}]", raw));
            }
        }
    }
    labels
}
