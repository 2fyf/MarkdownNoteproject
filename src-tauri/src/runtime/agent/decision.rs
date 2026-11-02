//! Strongly-typed evidence-gate decisions.
//!
//! `FinalizeStatus` and `FinalizeRuntime` replace the previous free-form
//! `String` fields on `FinalizeDecision` so internal call sites stop relying on
//! magic constants. Serialization preserves the existing camelCase / kebab-case
//! strings consumed by the frontend (`finalizeGate.status`,
//! `finalizeGate.runtime`).

use serde::Serialize;

/// Result of an evidence sufficiency check.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FinalizeStatus {
    Answerable,
    #[serde(rename = "needs_more_evidence")]
    NeedsMoreEvidence,
    Insufficient,
}

impl FinalizeStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Answerable => "answerable",
            Self::NeedsMoreEvidence => "needs_more_evidence",
            Self::Insufficient => "insufficient",
        }
    }
}

impl std::fmt::Display for FinalizeStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Which gate produced a finalize decision. The serialized form matches the
/// strings written into `finalizeGate.runtime` historically, so the frontend
/// (and any persisted traces) keep reading the same values.
///
/// Two of these are genuine answerability **verdicts** (`M4LlmJudge`,
/// `UnifiedLoop` — an LLM actually decided the evidence was enough to answer);
/// the M3 gates are **seeds** (cheap rule/heuristic pre-checks that gather
/// initial evidence but do not, on their own, authorize an answer). See
/// [`FinalizeRuntime::is_verdict`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum FinalizeRuntime {
    #[serde(rename = "m3-rule-guard")]
    M3RuleGuard,
    #[serde(rename = "m3-heuristic-judge")]
    M3HeuristicJudge,
    #[serde(rename = "m4-llm-judge")]
    M4LlmJudge,
    #[serde(rename = "unified-loop")]
    UnifiedLoop,
}

impl FinalizeRuntime {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::M3RuleGuard => "m3-rule-guard",
            Self::M3HeuristicJudge => "m3-heuristic-judge",
            Self::M4LlmJudge => "m4-llm-judge",
            Self::UnifiedLoop => "unified-loop",
        }
    }

    /// Parse the serialized `finalizeGate.runtime` string back into the enum.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "m3-rule-guard" => Some(Self::M3RuleGuard),
            "m3-heuristic-judge" => Some(Self::M3HeuristicJudge),
            "m4-llm-judge" => Some(Self::M4LlmJudge),
            "unified-loop" => Some(Self::UnifiedLoop),
            _ => None,
        }
    }

    /// Whether this runtime represents a genuine "the evidence is sufficient to
    /// answer" verdict (an LLM made the call), as opposed to an M3 seed/heuristic
    /// pre-check. The single source of truth for "did a real gate clear this turn"
    /// across the M4 judge and the unified agent loop.
    pub fn is_verdict(self) -> bool {
        matches!(self, Self::M4LlmJudge | Self::UnifiedLoop)
    }
}

impl std::fmt::Display for FinalizeRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Bookkeeping for how many retrieval steps an agent run consumed.
///
/// This is the single source of truth across M3 and M4 loops; downstream code
/// (DB persistence, frontend `retrievalAttemptCount`) should read
/// `attempts.used` instead of recomputing from traces.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RetrievalAttempts {
    pub used: u32,
    pub max: u32,
    pub budget_exhausted: bool,
}

impl RetrievalAttempts {
    pub fn new(used: u32, max: u32, budget_exhausted: bool) -> Self {
        Self {
            used,
            max,
            budget_exhausted,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_serialises_to_expected_strings() {
        assert_eq!(
            serde_json::to_string(&FinalizeStatus::Answerable).unwrap(),
            "\"answerable\""
        );
        assert_eq!(
            serde_json::to_string(&FinalizeStatus::NeedsMoreEvidence).unwrap(),
            "\"needs_more_evidence\""
        );
        assert_eq!(
            serde_json::to_string(&FinalizeStatus::Insufficient).unwrap(),
            "\"insufficient\""
        );
    }

    #[test]
    fn runtime_serialises_to_kebab_case_legacy_strings() {
        assert_eq!(
            serde_json::to_string(&FinalizeRuntime::M3RuleGuard).unwrap(),
            "\"m3-rule-guard\""
        );
        assert_eq!(
            serde_json::to_string(&FinalizeRuntime::M3HeuristicJudge).unwrap(),
            "\"m3-heuristic-judge\""
        );
        assert_eq!(
            serde_json::to_string(&FinalizeRuntime::M4LlmJudge).unwrap(),
            "\"m4-llm-judge\""
        );
    }

    #[test]
    fn as_str_matches_serialization() {
        assert_eq!(FinalizeStatus::Answerable.as_str(), "answerable");
        assert_eq!(FinalizeRuntime::M4LlmJudge.as_str(), "m4-llm-judge");
        assert_eq!(FinalizeRuntime::UnifiedLoop.as_str(), "unified-loop");
    }

    #[test]
    fn parse_roundtrips_every_runtime() {
        for runtime in [
            FinalizeRuntime::M3RuleGuard,
            FinalizeRuntime::M3HeuristicJudge,
            FinalizeRuntime::M4LlmJudge,
            FinalizeRuntime::UnifiedLoop,
        ] {
            assert_eq!(FinalizeRuntime::parse(runtime.as_str()), Some(runtime));
        }
        assert_eq!(FinalizeRuntime::parse("nonsense"), None);
    }

    #[test]
    fn only_llm_and_unified_loop_are_verdicts() {
        // Verdict runtimes authorize an answer; M3 seeds do not.
        assert!(FinalizeRuntime::M4LlmJudge.is_verdict());
        assert!(FinalizeRuntime::UnifiedLoop.is_verdict());
        assert!(!FinalizeRuntime::M3RuleGuard.is_verdict());
        assert!(!FinalizeRuntime::M3HeuristicJudge.is_verdict());
    }
}
