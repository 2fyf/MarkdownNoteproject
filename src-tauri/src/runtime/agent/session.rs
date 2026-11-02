use std::collections::HashMap;
use std::sync::Mutex;

use super::memory::{
    preview_text, RecentTurnSummary, SectionContextMemory, SelectionContextMemory,
    SessionMemorySnapshot,
};

/// Maximum number of agent sessions whose working memory we retain.
/// Older entries are evicted on a least-recently-touched basis so a long-running
/// process that opens many sessions cannot leak unbounded memory through this map.
///
/// The map is keyed by `session_id` (an agent conversation), NOT by document:
/// a session is the natural owner of conversational working memory, and a single
/// document may be the focus of several independent sessions.
const MAX_TRACKED_DOCUMENTS: usize = 64;

#[derive(Default)]
pub struct AgentSessionStore {
    inner: Mutex<SessionStoreInner>,
}

#[derive(Default)]
struct SessionStoreInner {
    sessions: HashMap<String, AgentSessionState>,
    tick: u64,
}

#[derive(Clone, Debug, Default)]
struct AgentSessionState {
    provider_id: Option<String>,
    turns: Vec<RecentTurnSummary>,
    last_selection: Option<SelectionContextMemory>,
    last_sections: Option<SectionContextMemory>,
    /// Monotonically increasing counter used purely as an LRU stamp; touched on
    /// `record_turn` and `snapshot` so active documents stay resident.
    last_touch: u64,
}

impl SessionStoreInner {
    fn next_tick(&mut self) -> u64 {
        self.tick = self.tick.wrapping_add(1);
        self.tick
    }

    fn touch(&mut self, document_id: &str) {
        let tick = self.next_tick();
        if let Some(state) = self.sessions.get_mut(document_id) {
            state.last_touch = tick;
        }
    }

    fn evict_if_needed(&mut self) {
        while self.sessions.len() > MAX_TRACKED_DOCUMENTS {
            let victim = self
                .sessions
                .iter()
                .min_by_key(|(_, state)| state.last_touch)
                .map(|(key, _)| key.clone());
            match victim {
                Some(key) => {
                    self.sessions.remove(&key);
                }
                None => break,
            }
        }
    }
}

impl AgentSessionStore {
    pub fn snapshot(&self, document_id: &str) -> Option<SessionMemorySnapshot> {
        let mut inner = self.inner.lock().ok()?;
        let state = inner.sessions.get(document_id)?.clone();
        inner.touch(document_id);
        Some(SessionMemorySnapshot {
            document_id: document_id.to_string(),
            provider_id: state.provider_id,
            recent_turns: state.turns,
            selection: state.last_selection,
            sections: state.last_sections,
        })
    }

    pub fn record_turn(
        &self,
        document_id: &str,
        provider_id: Option<String>,
        mut turn: RecentTurnSummary,
    ) {
        if let Some(selected_text_preview) = &turn.selected_text_preview {
            turn.selected_text_preview = Some(preview_text(selected_text_preview, 200));
        }

        if let Ok(mut inner) = self.inner.lock() {
            let tick = inner.next_tick();
            let state = inner.sessions.entry(document_id.to_string()).or_default();
            state.provider_id = provider_id;
            state.last_selection =
                turn.selected_text_preview
                    .as_ref()
                    .map(|text| SelectionContextMemory {
                        text_preview: text.clone(),
                        page: turn.citations.first().map(|citation| citation.page),
                    });
            state.last_sections = if turn.tree_titles.is_empty() {
                None
            } else {
                Some(SectionContextMemory {
                    titles: turn.tree_titles.clone(),
                })
            };
            state.turns.push(turn);
            if state.turns.len() > 12 {
                let drain_count = state.turns.len().saturating_sub(12);
                state.turns.drain(0..drain_count);
            }
            state.last_touch = tick;
            inner.evict_if_needed();
        }
    }

    pub fn clear_session(&self, session_id: &str) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.sessions.remove(session_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_turn(question: &str) -> RecentTurnSummary {
        RecentTurnSummary {
            question: question.to_string(),
            answer_preview: String::new(),
            citations: Vec::new(),
            selected_text_preview: None,
            tree_titles: Vec::new(),
        }
    }

    #[test]
    fn evicts_least_recently_touched_document_when_over_capacity() {
        let store = AgentSessionStore::default();
        for idx in 0..(MAX_TRACKED_DOCUMENTS + 5) {
            let doc_id = format!("doc-{idx}");
            store.record_turn(&doc_id, None, make_turn("hi"));
        }
        let inner = store.inner.lock().expect("lock");
        assert_eq!(inner.sessions.len(), MAX_TRACKED_DOCUMENTS);
        for idx in 0..5 {
            assert!(
                !inner.sessions.contains_key(&format!("doc-{idx}")),
                "oldest doc-{idx} should have been evicted"
            );
        }
        assert!(inner
            .sessions
            .contains_key(&format!("doc-{}", MAX_TRACKED_DOCUMENTS + 4)));
    }

    #[test]
    fn snapshot_refreshes_lru_so_active_doc_is_not_evicted() {
        let store = AgentSessionStore::default();
        for idx in 0..MAX_TRACKED_DOCUMENTS {
            store.record_turn(&format!("doc-{idx}"), None, make_turn("hi"));
        }
        // doc-0 was the oldest; touching it via snapshot should save it from
        // eviction when a new document arrives.
        let _ = store.snapshot("doc-0");
        store.record_turn("doc-new", None, make_turn("hi"));

        let inner = store.inner.lock().expect("lock");
        assert_eq!(inner.sessions.len(), MAX_TRACKED_DOCUMENTS);
        assert!(inner.sessions.contains_key("doc-0"));
        assert!(inner.sessions.contains_key("doc-new"));
        assert!(!inner.sessions.contains_key("doc-1"));
    }

    #[test]
    fn turns_per_document_capped_at_twelve() {
        let store = AgentSessionStore::default();
        for idx in 0..20 {
            store.record_turn("doc", None, make_turn(&format!("q-{idx}")));
        }
        let snapshot = store.snapshot("doc").expect("snapshot");
        assert_eq!(snapshot.recent_turns.len(), 12);
        assert_eq!(snapshot.recent_turns.first().unwrap().question, "q-8");
        assert_eq!(snapshot.recent_turns.last().unwrap().question, "q-19");
    }
}
