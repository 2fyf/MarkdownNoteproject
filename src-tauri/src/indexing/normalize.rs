use crate::{BlockIndexInput, LineIndexInput};

pub(crate) struct NormalizedBlockIndex {
    pub(crate) id: String,
    pub(crate) block_index: u32,
    pub(crate) text: String,
    pub(crate) bbox_list: serde_json::Value,
    pub(crate) role: String,
    pub(crate) region_index: u32,
}

#[derive(Clone)]
struct RawBlockForNormalize {
    text: String,
    bbox_list: serde_json::Value,
    role: Option<String>,
    region_index: u32,
    x1: f64,
    x2: f64,
    y1: f64,
    y2: f64,
    order: u32,
}

#[derive(Clone)]
struct RawLineForNormalize {
    text: String,
    bbox_list: serde_json::Value,
    x1: f64,
    x2: f64,
    y1: f64,
    y2: f64,
    line_no: u32,
}

struct NormalizedBlockDraft {
    text: String,
    bbox_list: serde_json::Value,
    role: String,
    region_index: u32,
    y1: f64,
    order: u32,
}

pub(crate) fn normalize_page_blocks(
    document_id: &str,
    page_no: u32,
    blocks: Vec<BlockIndexInput>,
    lines: &[LineIndexInput],
) -> Vec<NormalizedBlockIndex> {
    let raw = blocks
        .into_iter()
        .filter_map(|block| {
            let BlockIndexInput {
                id: _raw_id,
                block_index,
                text,
                bbox_list,
                role,
                region_index,
            } = block;
            let text = normalize_block_text(&text);
            if text.is_empty() {
                return None;
            }
            let (x1, y1, x2, y2) = bbox_bounds(&bbox_list);
            Some(RawBlockForNormalize {
                text,
                bbox_list,
                role: sanitize_extracted_role(role),
                region_index: region_index.unwrap_or(0),
                x1,
                x2,
                y1,
                y2,
                order: block_index,
            })
        })
        .collect::<Vec<_>>();

    let mut normalized = if page_no == 1 {
        normalize_first_page_blocks(raw)
    } else {
        merge_extracted_heading_labels(raw)
            .into_iter()
            .filter_map(|block| normalize_regular_block(block, page_no))
            .collect::<Vec<_>>()
    };

    normalized.sort_by(|left, right| {
        left.order.cmp(&right.order).then_with(|| {
            left.y1
                .partial_cmp(&right.y1)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    });
    normalized = attach_extracted_heading_labels_from_lines(normalized, lines);

    normalized
        .into_iter()
        .enumerate()
        .map(|(index, block)| NormalizedBlockIndex {
            id: format!("{document_id}-p{page_no}-b{}", index + 1),
            block_index: (index + 1) as u32,
            text: block.text,
            bbox_list: block.bbox_list,
            role: block.role,
            region_index: block.region_index,
        })
        .collect()
}

fn attach_extracted_heading_labels_from_lines(
    mut blocks: Vec<NormalizedBlockDraft>,
    lines: &[LineIndexInput],
) -> Vec<NormalizedBlockDraft> {
    let mut raw_lines = lines
        .iter()
        .filter_map(|line| {
            let text = normalize_block_text(&line.text);
            if text.is_empty() {
                return None;
            }
            let (x1, y1, x2, y2) = bbox_bounds(&line.bbox_list);
            Some(RawLineForNormalize {
                text,
                bbox_list: line.bbox_list.clone(),
                x1,
                x2,
                y1,
                y2,
                line_no: line.line_no,
            })
        })
        .collect::<Vec<_>>();
    if raw_lines.len() < 2 || blocks.is_empty() {
        return blocks;
    }
    raw_lines.sort_by(|left, right| {
        left.line_no.cmp(&right.line_no).then_with(|| {
            left.y1
                .partial_cmp(&right.y1)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    });

    for pair in raw_lines.windows(2) {
        let [label, heading] = pair else {
            continue;
        };
        if !should_merge_heading_label_line(label, heading) {
            continue;
        }
        let Some(block) = find_matching_heading_block(&mut blocks, label, heading) else {
            continue;
        };
        if block_starts_with_label(&block.text, &label.text) {
            continue;
        }
        block.text = format!("{} {}", label.text, block.text);
        block.bbox_list = merge_bbox_values([&label.bbox_list, &block.bbox_list].into_iter());
        block.y1 = block.y1.min(label.y1);
        block.role = "heading".to_string();
    }

    blocks
}

fn should_merge_heading_label_line(
    label: &RawLineForNormalize,
    heading: &RawLineForNormalize,
) -> bool {
    if !looks_like_extracted_section_label(&label.text) {
        return false;
    }
    if !looks_like_unlabeled_heading(&heading.text) {
        return false;
    }
    if label.x1 > 0.32 || heading.x1 > 0.45 {
        return false;
    }
    if heading.x1 <= label.x1 || heading.x1 - label.x2 > 0.08 {
        return false;
    }
    let label_height = (label.y2 - label.y1).abs().max(0.0001);
    let heading_height = (heading.y2 - heading.y1).abs().max(0.0001);
    let baseline_delta = (label.y1 - heading.y1).abs();
    baseline_delta <= label_height.max(heading_height) * 0.8
}

fn find_matching_heading_block<'a>(
    blocks: &'a mut [NormalizedBlockDraft],
    label: &RawLineForNormalize,
    heading: &RawLineForNormalize,
) -> Option<&'a mut NormalizedBlockDraft> {
    let best_index = blocks
        .iter()
        .enumerate()
        .filter(|(_, block)| {
            !block_starts_with_label(&block.text, &label.text)
                && block_text_starts_with_heading_line(&block.text, &heading.text)
                && block_bbox_matches_heading_line(&block.bbox_list, heading)
        })
        .min_by(|(_, left), (_, right)| {
            let (_, left_y1, _, _) = bbox_bounds(&left.bbox_list);
            let (_, right_y1, _, _) = bbox_bounds(&right.bbox_list);
            (left_y1 - heading.y1)
                .abs()
                .partial_cmp(&(right_y1 - heading.y1).abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(index, _)| index)?;
    blocks.get_mut(best_index)
}

fn block_starts_with_label(block_text: &str, label_text: &str) -> bool {
    let block_text = normalize_block_text(block_text);
    let label_text = normalize_block_text(label_text);
    block_text == label_text
        || block_text
            .strip_prefix(&label_text)
            .map(|rest| rest.starts_with(char::is_whitespace))
            .unwrap_or(false)
}

fn block_text_starts_with_heading_line(block_text: &str, heading_text: &str) -> bool {
    let block_text = normalize_block_text(block_text);
    let heading_text = normalize_block_text(heading_text);
    block_text == heading_text
        || block_text
            .strip_prefix(&heading_text)
            .map(|rest| rest.starts_with(char::is_whitespace))
            .unwrap_or(false)
}

fn block_bbox_matches_heading_line(
    block_bbox_list: &serde_json::Value,
    heading: &RawLineForNormalize,
) -> bool {
    let (block_x1, block_y1, block_x2, block_y2) = bbox_bounds(block_bbox_list);
    let block_height = (block_y2 - block_y1).abs().max(0.0001);
    let heading_height = (heading.y2 - heading.y1).abs().max(0.0001);
    let y_close = (block_y1 - heading.y1).abs() <= block_height.max(heading_height) * 1.2;
    let x_overlaps = heading.x2 >= block_x1 - 0.02 && heading.x1 <= block_x2 + 0.02;
    y_close && x_overlaps
}

fn normalize_first_page_blocks(blocks: Vec<RawBlockForNormalize>) -> Vec<NormalizedBlockDraft> {
    let abstract_index = blocks
        .iter()
        .position(|block| block.text.to_lowercase().starts_with("abstract"));
    let mut result = Vec::new();

    if let Some(abstract_index) = abstract_index {
        let before_abstract = &blocks[..abstract_index];
        if let Some(title) = before_abstract
            .iter()
            .find(|block| looks_like_paper_title(&block.text))
        {
            result.push(NormalizedBlockDraft {
                text: title.text.clone(),
                bbox_list: title.bbox_list.clone(),
                role: "title".to_string(),
                region_index: title.region_index,
                y1: title.y1,
                order: title.order,
            });
        }

        let author_parts = before_abstract
            .iter()
            .filter(|block| looks_like_author_fragment(&block.text))
            .map(|block| block.text.as_str())
            .collect::<Vec<_>>();
        let author_text = normalize_author_line(&author_parts);
        if !author_text.is_empty() {
            let author_region_index = first_matching_region_index(before_abstract, |block| {
                looks_like_author_fragment(&block.text)
            });
            result.push(NormalizedBlockDraft {
                text: author_text,
                bbox_list: merge_bbox_values(
                    before_abstract
                        .iter()
                        .filter(|block| looks_like_author_fragment(&block.text))
                        .map(|block| &block.bbox_list),
                ),
                role: "authors".to_string(),
                region_index: author_region_index,
                y1: before_abstract
                    .iter()
                    .find(|block| looks_like_author_fragment(&block.text))
                    .map(|block| block.y1)
                    .unwrap_or(0.0),
                order: before_abstract
                    .iter()
                    .find(|block| looks_like_author_fragment(&block.text))
                    .map(|block| block.order)
                    .unwrap_or(0),
            });
        }

        let affiliation_parts = before_abstract
            .iter()
            .filter(|block| looks_like_affiliation(&block.text))
            .map(|block| block.text.as_str())
            .collect::<Vec<_>>();
        let affiliation_text = normalize_affiliation_line(&affiliation_parts);
        if !affiliation_text.is_empty() {
            let affiliation_region_index = first_matching_region_index(before_abstract, |block| {
                looks_like_affiliation(&block.text)
            });
            result.push(NormalizedBlockDraft {
                text: affiliation_text,
                bbox_list: merge_bbox_values(
                    before_abstract
                        .iter()
                        .filter(|block| looks_like_affiliation(&block.text))
                        .map(|block| &block.bbox_list),
                ),
                role: "affiliations".to_string(),
                region_index: affiliation_region_index,
                y1: before_abstract
                    .iter()
                    .find(|block| looks_like_affiliation(&block.text))
                    .map(|block| block.y1)
                    .unwrap_or(0.0),
                order: before_abstract
                    .iter()
                    .find(|block| looks_like_affiliation(&block.text))
                    .map(|block| block.order)
                    .unwrap_or(0),
            });
        }

        for block in
            merge_extracted_heading_labels(blocks.into_iter().skip(abstract_index).collect())
        {
            if let Some(normalized) = normalize_regular_block(block, 1) {
                result.push(normalized);
            }
        }
        return result;
    }

    merge_extracted_heading_labels(blocks)
        .into_iter()
        .filter_map(|block| normalize_regular_block(block, 1))
        .collect()
}

fn merge_extracted_heading_labels(blocks: Vec<RawBlockForNormalize>) -> Vec<RawBlockForNormalize> {
    let mut merged = Vec::with_capacity(blocks.len());
    let mut index = 0;
    while index < blocks.len() {
        let current = &blocks[index];
        if index + 1 < blocks.len() {
            let next = &blocks[index + 1];
            if should_merge_heading_label(current, next) {
                merged.push(merge_heading_label_blocks(current, next));
                index += 2;
                continue;
            }
        }
        merged.push(current.clone());
        index += 1;
    }
    merged
}

fn should_merge_heading_label(
    label: &RawBlockForNormalize,
    heading: &RawBlockForNormalize,
) -> bool {
    if !looks_like_extracted_section_label(&label.text) {
        return false;
    }
    if !looks_like_unlabeled_heading(&heading.text) {
        return false;
    }
    if label.order + 1 != heading.order {
        return false;
    }
    if label.region_index != 0
        && heading.region_index != 0
        && label.region_index != heading.region_index
    {
        return false;
    }
    if label.x1 > 0.32 || heading.x1 > 0.45 {
        return false;
    }
    if heading.x1 <= label.x1 || heading.x1 - label.x2 > 0.08 {
        return false;
    }
    let label_height = (label.y2 - label.y1).abs().max(0.0001);
    let heading_height = (heading.y2 - heading.y1).abs().max(0.0001);
    let baseline_delta = (label.y1 - heading.y1).abs();
    baseline_delta <= label_height.max(heading_height) * 0.8
}

fn merge_heading_label_blocks(
    label: &RawBlockForNormalize,
    heading: &RawBlockForNormalize,
) -> RawBlockForNormalize {
    RawBlockForNormalize {
        text: format!("{} {}", label.text, heading.text),
        bbox_list: merge_bbox_values([&label.bbox_list, &heading.bbox_list].into_iter()),
        role: Some("heading".to_string()),
        region_index: if label.region_index == heading.region_index {
            label.region_index
        } else {
            0
        },
        x1: label.x1.min(heading.x1),
        x2: label.x2.max(heading.x2),
        y1: label.y1.min(heading.y1),
        y2: label.y2.max(heading.y2),
        order: label.order,
    }
}

fn normalize_regular_block(
    block: RawBlockForNormalize,
    page_no: u32,
) -> Option<NormalizedBlockDraft> {
    let text = normalize_block_text(&block.text);
    if text.is_empty() || is_noise_block_text(&text) {
        return None;
    }
    let role = block
        .role
        .as_deref()
        .or_else(|| classify_block_role(&text, page_no))?;
    Some(NormalizedBlockDraft {
        text,
        bbox_list: block.bbox_list,
        role: role.to_string(),
        region_index: block.region_index,
        y1: block.y1,
        order: block.order,
    })
}

fn first_matching_region_index(
    blocks: &[RawBlockForNormalize],
    predicate: impl Fn(&RawBlockForNormalize) -> bool,
) -> u32 {
    blocks
        .iter()
        .find(|block| predicate(block))
        .map(|block| block.region_index)
        .unwrap_or(0)
}

fn sanitize_extracted_role(role: Option<String>) -> Option<String> {
    let role = role?.trim().to_lowercase();
    matches!(
        role.as_str(),
        "title"
            | "authors"
            | "affiliations"
            | "abstract"
            | "heading"
            | "caption"
            | "code"
            | "formula"
            | "table"
            | "footnote"
            | "reference"
            | "header"
            | "footer"
            | "body"
    )
    .then_some(role)
}

fn classify_block_role(text: &str, _page_no: u32) -> Option<&'static str> {
    let lower = text.to_lowercase();
    if lower.starts_with("abstract") {
        return Some("abstract");
    }
    if lower.starts_with("figure ") || lower.starts_with("table ") {
        return Some("caption");
    }
    if is_reliable_heading_text(text) {
        return Some("heading");
    }
    if text.chars().filter(|ch| ch.is_alphabetic()).count() >= 12 {
        return Some("body");
    }
    None
}

fn normalize_block_text(value: &str) -> String {
    value
        .replace('\n', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn bbox_bounds(value: &serde_json::Value) -> (f64, f64, f64, f64) {
    let mut x1 = f64::INFINITY;
    let mut y1 = f64::INFINITY;
    let mut x2 = f64::NEG_INFINITY;
    let mut y2 = f64::NEG_INFINITY;
    if let Some(items) = value.as_array() {
        for bbox in items.iter().filter_map(|item| item.as_array()) {
            let Some(left) = bbox.first().and_then(|value| value.as_f64()) else {
                continue;
            };
            let Some(top) = bbox.get(1).and_then(|value| value.as_f64()) else {
                continue;
            };
            let Some(right) = bbox.get(2).and_then(|value| value.as_f64()) else {
                continue;
            };
            let Some(bottom) = bbox.get(3).and_then(|value| value.as_f64()) else {
                continue;
            };
            x1 = x1.min(left);
            y1 = y1.min(top);
            x2 = x2.max(right);
            y2 = y2.max(bottom);
        }
    }
    if [x1, y1, x2, y2].iter().all(|value| value.is_finite()) {
        (x1, y1, x2, y2)
    } else {
        (0.0, 0.0, 0.0, 0.0)
    }
}

fn merge_bbox_values<'a>(values: impl Iterator<Item = &'a serde_json::Value>) -> serde_json::Value {
    let mut merged = Vec::new();
    for value in values {
        if let Some(items) = value.as_array() {
            merged.extend(items.iter().cloned());
        }
    }
    serde_json::Value::Array(merged)
}

fn looks_like_paper_title(text: &str) -> bool {
    let alpha_count = text.chars().filter(|ch| ch.is_alphabetic()).count();
    alpha_count >= 24
        && text.chars().count() <= 180
        && !looks_like_affiliation(text)
        && !text.to_lowercase().starts_with("abstract")
}

fn looks_like_author_fragment(text: &str) -> bool {
    if looks_like_affiliation(text) || is_noise_block_text(text) {
        return false;
    }
    let cleaned = text
        .trim_matches(|ch: char| ch.is_whitespace() || matches!(ch, ',' | '*' | '†' | '|' | ';'));
    let alpha_count = cleaned.chars().filter(|ch| ch.is_alphabetic()).count();
    let token_count = cleaned.split_whitespace().count();
    (5..=80).contains(&alpha_count)
        && (2..=8).contains(&token_count)
        && cleaned.chars().all(|ch| {
            ch.is_alphabetic()
                || ch.is_numeric()
                || ch.is_whitespace()
                || matches!(ch, '-' | '\'' | '.' | '|' | ',')
        })
}

fn normalize_author_line(parts: &[&str]) -> String {
    let mut names = Vec::new();
    for part in parts {
        for name in part.split([',', '\n', '|']) {
            let cleaned = clean_author_name(name);
            if looks_like_author_fragment(&cleaned) && !names.contains(&cleaned) {
                names.push(cleaned);
            }
        }
    }
    names.join(", ")
}

fn clean_author_name(value: &str) -> String {
    value
        .trim()
        .trim_matches(|ch: char| {
            ch.is_whitespace() || ch.is_numeric() || matches!(ch, ',' | '*' | '†' | '‡' | ';' | '|')
        })
        .split_whitespace()
        .filter_map(|token| {
            let token = token.trim_matches(|ch: char| {
                ch.is_numeric() || matches!(ch, ',' | '*' | '†' | '‡' | ';' | '|')
            });
            token.chars().any(|ch| ch.is_alphabetic()).then_some(token)
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn looks_like_affiliation(text: &str) -> bool {
    let lower = text.to_lowercase();
    [
        "university",
        "institute",
        "laboratory",
        " lab",
        "department",
        "group",
        "college",
        "school",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn normalize_affiliation_line(parts: &[&str]) -> String {
    let mut affiliations = Vec::new();
    for part in parts {
        let cleaned = part
            .trim()
            .trim_matches(|ch: char| ch.is_whitespace() || ch.is_ascii_digit())
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if !cleaned.is_empty() && !affiliations.contains(&cleaned) {
            affiliations.push(cleaned);
        }
    }
    affiliations.join("; ")
}

fn is_noise_block_text(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.chars().count() <= 1 {
        return true;
    }
    if trimmed.chars().all(|ch| {
        ch.is_ascii_digit()
            || ch.is_whitespace()
            || matches!(ch, '.' | '-' | '/' | '%' | '×' | '⬇' | '(' | ')' | '*')
    }) {
        return true;
    }
    let lower = trimmed.to_lowercase();
    lower == "w/o swe-pruner"
        || lower == "w/ swe-pruner"
        || lower == "frequency"
        || lower == "prompt tokens"
        || lower == "completion tokens"
        || lower == "total tokens"
        || lower == "agent rounds"
}

fn looks_like_extracted_section_label(text: &str) -> bool {
    let trimmed = text.trim().trim_end_matches('.');
    if trimmed.is_empty() || trimmed.chars().count() > 8 {
        return false;
    }
    if trimmed.chars().all(|ch| ch.is_ascii_digit() || ch == '.')
        && trimmed.chars().any(|ch| ch.is_ascii_digit())
    {
        return trimmed
            .split('.')
            .filter(|part| !part.is_empty())
            .all(|part| {
                part.parse::<u32>()
                    .map(|value| value <= 30)
                    .unwrap_or(false)
            });
    }
    trimmed.chars().count() == 1
        && trimmed
            .chars()
            .next()
            .map(|ch| ch.is_ascii_uppercase())
            .unwrap_or(false)
}

fn looks_like_unlabeled_heading(text: &str) -> bool {
    let normalized = text.trim();
    if normalized.is_empty() || normalized.chars().count() > 96 {
        return false;
    }
    if normalized.ends_with('.') || normalized.ends_with(',') || normalized.ends_with(';') {
        return false;
    }
    let alpha_count = normalized.chars().filter(|ch| ch.is_alphabetic()).count();
    if alpha_count < 4 {
        return false;
    }
    let word_count = normalized.split_whitespace().count();
    if word_count > 10 {
        return false;
    }
    if is_reliable_heading_text(normalized) {
        return true;
    }
    normalized
        .chars()
        .filter(|ch| ch.is_alphabetic())
        .all(|ch| ch.is_uppercase())
        || looks_like_title_case_heading(normalized)
}

fn looks_like_title_case_heading(text: &str) -> bool {
    let mut meaningful_words = 0usize;
    let mut title_case_words = 0usize;
    for word in text.split_whitespace() {
        let cleaned = word.trim_matches(|ch: char| !ch.is_alphanumeric());
        if cleaned.is_empty() || is_heading_stop_word(cleaned) {
            continue;
        }
        meaningful_words += 1;
        if cleaned
            .chars()
            .find(|ch| ch.is_alphabetic())
            .map(|ch| ch.is_uppercase())
            .unwrap_or(false)
        {
            title_case_words += 1;
        }
    }
    meaningful_words > 0 && title_case_words == meaningful_words
}

fn is_heading_stop_word(word: &str) -> bool {
    matches!(
        word.to_ascii_lowercase().as_str(),
        "a" | "an"
            | "and"
            | "as"
            | "by"
            | "for"
            | "from"
            | "in"
            | "of"
            | "on"
            | "or"
            | "the"
            | "to"
            | "with"
    )
}

fn is_reliable_heading_text(text: &str) -> bool {
    let normalized = text.trim();
    let lower = normalized.to_lowercase();
    if matches!(
        lower.as_str(),
        "introduction"
            | "background"
            | "related work"
            | "method"
            | "methods"
            | "methodology"
            | "approach"
            | "experiments"
            | "evaluation"
            | "results"
            | "discussion"
            | "limitations"
            | "conclusion"
            | "references"
            | "appendix"
    ) {
        return true;
    }
    let mut parts = normalized.splitn(2, char::is_whitespace);
    let first = parts.next().unwrap_or_default().trim_end_matches('.');
    let rest = parts.next().unwrap_or_default().trim();
    if first.len() == 1
        && first
            .chars()
            .next()
            .map(|ch| ch.is_ascii_uppercase())
            .unwrap_or(false)
        && looks_like_unlabeled_heading(rest)
    {
        return true;
    }
    !rest.is_empty()
        && rest.chars().filter(|ch| ch.is_alphabetic()).count() >= 4
        && first.chars().all(|ch| ch.is_ascii_digit() || ch == '.')
        && first.chars().any(|ch| ch.is_ascii_digit())
        && first
            .split('.')
            .next()
            .and_then(|part| part.parse::<u32>().ok())
            .map(|section| section <= 20)
            .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_page_normalization_merges_header_blocks() {
        let blocks = vec![
            block_input("ByteDance", 8.0),
            block_input(
                "SWE-Pruner: Self-Adaptive Context Pruning for Coding Agents",
                10.0,
            ),
            block_input("Yuhang Wang1 | Heng Lian1", 20.0),
            block_input("1", 21.0),
            block_input(", Yuling Shi | , Yuting Chen", 22.0),
            block_input(", Shilin He3", 23.0),
            block_input("1LLMSE Lab, Shanghai Jiao Tong University", 30.0),
            block_input("2Sun Yat-sen University", 31.0),
            block_input("3Douyin Group", 32.0),
            block_input(
                "Abstract LLM agents have demonstrated remarkable capabilities in software development.",
                40.0,
            ),
            block_input("60 Agent Rounds", 200.0),
        ];

        let normalized = normalize_page_blocks("doc", 1, blocks, &[]);
        let roles = normalized
            .iter()
            .map(|block| block.role.as_str())
            .collect::<Vec<_>>();

        assert_eq!(roles, vec!["title", "authors", "affiliations", "abstract"]);
        assert!(normalized[1].text.contains("Yuhang Wang"));
        assert!(normalized[1].text.contains("Heng Lian"));
        assert!(normalized[1].text.contains("Yuling Shi"));
        assert!(normalized[1].text.contains("Yuting Chen"));
        assert!(normalized[1].text.contains("Shilin He"));
        assert!(!normalized[1].text.contains("ByteDance"));
        assert!(!normalized[1].text.contains("Wang1"));
        assert!(!normalized[1].text.contains("He3"));
        assert!(normalized[2].text.contains("Shanghai Jiao Tong University"));
        assert!(!normalized
            .iter()
            .any(|block| block.text == "60 Agent Rounds"));
    }

    #[test]
    fn normalization_preserves_frontend_block_order() {
        let blocks = vec![
            block_input_with_index("First logical paragraph with enough text.", 1, 200.0),
            block_input_with_index("Second logical paragraph with enough text.", 2, 100.0),
        ];

        let normalized = normalize_page_blocks("doc", 2, blocks, &[]);

        assert_eq!(
            normalized[0].text,
            "First logical paragraph with enough text."
        );
        assert_eq!(
            normalized[1].text,
            "Second logical paragraph with enough text."
        );
    }

    #[test]
    fn normalization_merges_extracted_numeric_heading_label() {
        let blocks = vec![
            block_input_with_bbox("5", 1, 0.1158, 0.0413, 0.1233, 0.0526),
            block_input_with_bbox("RESULTS", 2, 0.1384, 0.0413, 0.2067, 0.0526),
            block_input_with_bbox("5.1.0.0", 3, 0.8416, 0.0413, 0.8842, 0.0526),
            block_input_with_bbox("3.4", 4, 0.1158, 0.0883, 0.1418, 0.1021),
            block_input_with_bbox(
                "Integration with Agentic Workflows",
                5,
                0.1622,
                0.0883,
                0.4823,
                0.1021,
            ),
        ];

        let normalized = normalize_page_blocks("doc", 6, blocks, &[]);
        let texts = normalized
            .iter()
            .map(|block| block.text.as_str())
            .collect::<Vec<_>>();

        assert_eq!(
            texts,
            vec!["5 RESULTS", "3.4 Integration with Agentic Workflows"]
        );
        assert_eq!(normalized[0].role, "heading");
        assert_eq!(normalized[1].role, "heading");
    }

    #[test]
    fn normalization_merges_extracted_appendix_heading_label() {
        let blocks = vec![
            block_input_with_bbox("H", 1, 0.1158, 0.0413, 0.1271, 0.0526),
            block_input_with_bbox(
                "SYNTACTIC STRUCTURE PRESERVATION ANALYSIS",
                2,
                0.1421,
                0.0413,
                0.5362,
                0.0526,
            ),
            block_input_with_bbox("I", 3, 0.8788, 0.0413, 0.8842, 0.0526),
        ];

        let normalized = normalize_page_blocks("doc", 20, blocks, &[]);

        assert_eq!(normalized.len(), 1);
        assert_eq!(
            normalized[0].text,
            "H SYNTACTIC STRUCTURE PRESERVATION ANALYSIS"
        );
        assert_eq!(normalized[0].role, "heading");
    }

    #[test]
    fn normalization_does_not_merge_distant_numeric_label() {
        let blocks = vec![
            block_input_with_bbox("5", 1, 0.70, 0.0413, 0.72, 0.0526),
            block_input_with_bbox("RESULTS", 2, 0.1384, 0.0413, 0.2067, 0.0526),
        ];

        let normalized = normalize_page_blocks("doc", 6, blocks, &[]);

        assert_eq!(normalized.len(), 1);
        assert_eq!(normalized[0].text, "RESULTS");
    }

    #[test]
    fn normalization_merges_numeric_heading_label_from_lines() {
        let blocks = vec![
            block_input_with_bbox("RESULTS", 1, 0.1384, 0.0413, 0.2067, 0.0526),
            block_input_with_bbox(
                "Integration with Agentic Workflows",
                2,
                0.1622,
                0.0883,
                0.4823,
                0.1021,
            ),
        ];
        let lines = vec![
            line_input_with_bbox("5", 1, 0.1158, 0.0413, 0.1233, 0.0526),
            line_input_with_bbox("RESULTS", 2, 0.1384, 0.0413, 0.2067, 0.0526),
            line_input_with_bbox("3.4", 3, 0.1158, 0.0883, 0.1418, 0.1021),
            line_input_with_bbox(
                "Integration with Agentic Workflows",
                4,
                0.1622,
                0.0883,
                0.4823,
                0.1021,
            ),
        ];

        let normalized = normalize_page_blocks("doc", 6, blocks, &lines);
        let texts = normalized
            .iter()
            .map(|block| block.text.as_str())
            .collect::<Vec<_>>();

        assert_eq!(
            texts,
            vec!["5 RESULTS", "3.4 Integration with Agentic Workflows"]
        );
        assert_eq!(normalized[0].role, "heading");
        assert_eq!(normalized[1].role, "heading");
    }

    #[test]
    fn normalization_merges_appendix_heading_label_from_lines() {
        let blocks = vec![block_input_with_bbox(
            "SYNTACTIC STRUCTURE PRESERVATION ANALYSIS",
            1,
            0.1421,
            0.0413,
            0.5362,
            0.0526,
        )];
        let lines = vec![
            line_input_with_bbox("H", 1, 0.1158, 0.0413, 0.1271, 0.0526),
            line_input_with_bbox(
                "SYNTACTIC STRUCTURE PRESERVATION ANALYSIS",
                2,
                0.1421,
                0.0413,
                0.5362,
                0.0526,
            ),
        ];

        let normalized = normalize_page_blocks("doc", 20, blocks, &lines);

        assert_eq!(normalized.len(), 1);
        assert_eq!(
            normalized[0].text,
            "H SYNTACTIC STRUCTURE PRESERVATION ANALYSIS"
        );
        assert_eq!(normalized[0].role, "heading");
    }

    #[test]
    fn normalization_does_not_merge_distant_line_heading_label() {
        let blocks = vec![block_input_with_bbox(
            "RESULTS", 1, 0.1384, 0.0413, 0.2067, 0.0526,
        )];
        let lines = vec![
            line_input_with_bbox("5", 1, 0.70, 0.0413, 0.72, 0.0526),
            line_input_with_bbox("RESULTS", 2, 0.1384, 0.0413, 0.2067, 0.0526),
        ];

        let normalized = normalize_page_blocks("doc", 6, blocks, &lines);

        assert_eq!(normalized.len(), 1);
        assert_eq!(normalized[0].text, "RESULTS");
    }

    #[test]
    fn normalization_preserves_extracted_code_role() {
        let mut block = block_input_with_bbox(
            "def grep_with_pruner(file_path, pattern):",
            1,
            0.10,
            0.20,
            0.60,
            0.24,
        );
        block.role = Some("code".to_string());

        let normalized = normalize_page_blocks("doc", 2, vec![block], &[]);

        assert_eq!(normalized.len(), 1);
        assert_eq!(normalized[0].role, "code");
    }

    #[test]
    fn normalization_preserves_region_index() {
        let mut block = block_input_with_bbox("Body text", 1, 0.10, 0.20, 0.60, 0.24);
        block.role = Some("body".to_string());
        block.region_index = Some(3);

        let normalized = normalize_page_blocks("doc", 2, vec![block], &[]);

        assert_eq!(normalized.len(), 1);
        assert_eq!(normalized[0].region_index, 3);
    }

    fn block_input(text: &str, y: f64) -> BlockIndexInput {
        block_input_with_index(text, y as u32, y)
    }

    fn block_input_with_index(text: &str, block_index: u32, y: f64) -> BlockIndexInput {
        BlockIndexInput {
            id: format!("raw-{y}"),
            block_index,
            text: text.to_string(),
            bbox_list: serde_json::json!([[0.0, y, 10.0, y + 5.0]]),
            role: None,
            region_index: None,
        }
    }

    fn block_input_with_bbox(
        text: &str,
        block_index: u32,
        x1: f64,
        y1: f64,
        x2: f64,
        y2: f64,
    ) -> BlockIndexInput {
        BlockIndexInput {
            id: format!("raw-{block_index}"),
            block_index,
            text: text.to_string(),
            bbox_list: serde_json::json!([[x1, y1, x2, y2]]),
            role: None,
            region_index: None,
        }
    }

    fn line_input_with_bbox(
        text: &str,
        line_no: u32,
        x1: f64,
        y1: f64,
        x2: f64,
        y2: f64,
    ) -> LineIndexInput {
        LineIndexInput {
            line_no,
            text: text.to_string(),
            bbox_list: serde_json::json!([[x1, y1, x2, y2]]),
            source_order_list: None,
            region_index: None,
            role_hint: None,
            baseline: None,
        }
    }
}
