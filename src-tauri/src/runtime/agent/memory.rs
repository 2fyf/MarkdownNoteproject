use crate::runtime::rag::Citation;

#[derive(Clone, Debug)]
pub struct RecentCitationSummary {
    pub label: String,
    pub page: u32,
    pub quote: String,
}

#[derive(Clone, Debug)]
pub struct RecentTurnSummary {
    pub question: String,
    pub answer_preview: String,
    pub citations: Vec<RecentCitationSummary>,
    pub selected_text_preview: Option<String>,
    pub tree_titles: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct SelectionContextMemory {
    pub text_preview: String,
    pub page: Option<u32>,
}

#[derive(Clone, Debug)]
pub struct SectionContextMemory {
    pub titles: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct SessionMemorySnapshot {
    pub document_id: String,
    pub provider_id: Option<String>,
    pub recent_turns: Vec<RecentTurnSummary>,
    pub selection: Option<SelectionContextMemory>,
    pub sections: Option<SectionContextMemory>,
}

pub fn preview_text(value: &str, max_chars: usize) -> String {
    let mut text = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.chars().count() <= max_chars {
        return text;
    }
    text = text.chars().take(max_chars).collect::<String>();
    text.push_str("...");
    text
}

pub fn summarize_citations(citations: &[Citation], limit: usize) -> Vec<RecentCitationSummary> {
    citations
        .iter()
        .take(limit)
        .map(|citation| RecentCitationSummary {
            label: citation.label.clone(),
            page: citation.page,
            quote: preview_text(&citation.quote, 120),
        })
        .collect()
}
