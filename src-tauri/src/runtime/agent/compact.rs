use serde::Serialize;

use super::memory::{preview_text, RecentTurnSummary, SessionMemorySnapshot};

const RECENT_TURNS_TO_KEEP: usize = 3;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactResult {
    pub trigger: String,
    pub retained_recent_turns: usize,
    pub summarized_turns: usize,
    pub summary_generated: bool,
}

#[derive(Clone, Debug)]
pub struct SessionSummary {
    pub summary_text: Option<String>,
    pub recent_turns: Vec<RecentTurnSummary>,
    pub compact: CompactResult,
}

pub fn compact_session(snapshot: &SessionMemorySnapshot) -> SessionSummary {
    if snapshot.recent_turns.len() <= RECENT_TURNS_TO_KEEP {
        return SessionSummary {
            summary_text: None,
            recent_turns: snapshot.recent_turns.clone(),
            compact: CompactResult {
                trigger: "turn_start".to_string(),
                retained_recent_turns: snapshot.recent_turns.len(),
                summarized_turns: 0,
                summary_generated: false,
            },
        };
    }

    let split_index = snapshot.recent_turns.len() - RECENT_TURNS_TO_KEEP;
    let (older, recent) = snapshot.recent_turns.split_at(split_index);
    let summary_text = build_rule_summary(older);
    SessionSummary {
        summary_text: Some(summary_text.clone()),
        recent_turns: recent.to_vec(),
        compact: CompactResult {
            trigger: "turn_start".to_string(),
            retained_recent_turns: recent.len(),
            summarized_turns: older.len(),
            summary_generated: !summary_text.trim().is_empty(),
        },
    }
}

fn build_rule_summary(turns: &[RecentTurnSummary]) -> String {
    let mut lines = vec!["Session summary:".to_string()];
    for turn in turns.iter().rev().take(4).rev() {
        lines.push(format!("- Q: {}", preview_text(&turn.question, 120)));
        lines.push(format!("- A: {}", preview_text(&turn.answer_preview, 160)));
        if !turn.tree_titles.is_empty() {
            lines.push(format!("- Sections: {}", turn.tree_titles.join(", ")));
        }
        if !turn.citations.is_empty() {
            let citations = turn
                .citations
                .iter()
                .take(3)
                .map(|citation| {
                    format!(
                        "{} p{} ({})",
                        citation.label,
                        citation.page,
                        preview_text(&citation.quote, 48)
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            lines.push(format!("- Citations: {citations}"));
        }
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::agent::memory::SessionMemorySnapshot;

    #[test]
    fn compact_keeps_recent_turns_and_summarizes_older_ones() {
        let snapshot = SessionMemorySnapshot {
            document_id: "doc".to_string(),
            provider_id: None,
            recent_turns: (0..5)
                .map(|index| RecentTurnSummary {
                    question: format!("question {index}"),
                    answer_preview: format!("answer {index}"),
                    citations: Vec::new(),
                    selected_text_preview: None,
                    tree_titles: vec![format!("Section {index}")],
                })
                .collect(),
            selection: None,
            sections: None,
        };
        let compacted = compact_session(&snapshot);
        assert_eq!(compacted.recent_turns.len(), 3);
        assert_eq!(compacted.compact.summarized_turns, 2);
        assert!(compacted
            .summary_text
            .unwrap_or_default()
            .contains("Session summary:"));
    }
}
