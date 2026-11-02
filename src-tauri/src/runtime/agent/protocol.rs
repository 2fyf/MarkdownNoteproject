use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentIntent {
    Explain,
    Summarize,
    Locate,
    Compare,
}

impl AgentIntent {
    pub fn infer(question: &str) -> Self {
        let question = question.to_lowercase();
        if question.contains("compare") || question.contains("区别") || question.contains("对比")
        {
            Self::Compare
        } else if question.contains("where")
            || question.contains("在哪")
            || question.contains("位置")
        {
            Self::Locate
        } else if question.contains("summarize")
            || question.contains("总结")
            || question.contains("概括")
        {
            Self::Summarize
        } else {
            Self::Explain
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Explain => "explain",
            Self::Summarize => "summarize",
            Self::Locate => "locate",
            Self::Compare => "compare",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStepKind {
    SessionCompact,
    IntentClassify,
    InspectTree,
    OpenSection,
    SearchChunks,
    OpenPages,
    FinalizeAnswer,
}

impl AgentStepKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::SessionCompact => "session_compact",
            Self::IntentClassify => "intent_classify",
            Self::InspectTree => "inspect_tree",
            Self::OpenSection => "open_section",
            Self::SearchChunks => "search_chunks",
            Self::OpenPages => "open_pages",
            Self::FinalizeAnswer => "finalize_answer",
        }
    }
}
