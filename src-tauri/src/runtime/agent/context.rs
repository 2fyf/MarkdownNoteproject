use crate::runtime::rag::RetrievalRun;

use super::compact::SessionSummary;
use super::memory::SessionMemorySnapshot;

pub fn build_chat_context(
    snapshot: Option<&SessionMemorySnapshot>,
    compacted: &SessionSummary,
    retrieval_run: &RetrievalRun,
) -> String {
    let mut sections = Vec::new();

    if let Some(snapshot) = snapshot {
        let mut header = format!("Current document: {}", snapshot.document_id);
        if let Some(provider_id) = snapshot
            .provider_id
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            header.push_str(&format!(" | recent provider: {provider_id}"));
        }
        sections.push(header);
    }

    if let Some(summary) = compacted
        .summary_text
        .as_ref()
        .filter(|value| !value.trim().is_empty())
    {
        sections.push(summary.clone());
    }

    if !compacted.recent_turns.is_empty() {
        let mut recent_turns = String::from("Recent turns:");
        for turn in &compacted.recent_turns {
            recent_turns.push_str(&format!(
                "\n- Q: {}\n- A: {}",
                turn.question, turn.answer_preview
            ));
            if !turn.tree_titles.is_empty() {
                recent_turns.push_str(&format!("\n- Sections: {}", turn.tree_titles.join(", ")));
            }
        }
        sections.push(recent_turns);
    }

    if let Some(selection) = snapshot.and_then(|value| value.selection.as_ref()) {
        let mut selected = format!("Last selection: {}", selection.text_preview);
        if let Some(page) = selection.page {
            selected.push_str(&format!(" (page {page})"));
        }
        sections.push(selected);
    }

    if let Some(section_context) = snapshot.and_then(|value| value.sections.as_ref()) {
        if !section_context.titles.is_empty() {
            sections.push(format!(
                "Recent sections: {}",
                section_context.titles.join(", ")
            ));
        }
    }

    if !retrieval_run.intent.trim().is_empty() {
        sections.push(format!("Current intent: {}", retrieval_run.intent));
    }

    sections.join("\n\n")
}
