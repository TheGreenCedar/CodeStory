use crate::mode::RetrievalDegradedMode;
use crate::query_features::{QueryFeatures, QueryShape};
use serde::{Deserialize, Serialize};

/// Staged retrieval lane.
///
/// Stage labels are part of the trace contract consumed by runtime packet diagnostics. Repo-text
/// fallback is represented here for diagnostics only; full sidecar plans do not use it as product
/// evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetrievalStageKind {
    Stage0ScipAnchor,
    #[serde(alias = "stage1_zoekt_lexical")]
    Stage1Lexical,
    Stage1bQdrantSemantic,
    Stage2ScipExpand,
    Stage3RepoTextFallback,
}

impl RetrievalStageKind {
    pub fn label(self) -> &'static str {
        match self {
            RetrievalStageKind::Stage0ScipAnchor => "stage0_scip_anchor",
            RetrievalStageKind::Stage1Lexical => "stage1_lexical",
            RetrievalStageKind::Stage1bQdrantSemantic => "stage1b_qdrant_semantic",
            RetrievalStageKind::Stage2ScipExpand => "stage2_scip_expand",
            RetrievalStageKind::Stage3RepoTextFallback => "stage3_repo_text_fallback",
        }
    }

    pub fn provenance_label(self) -> Option<&'static str> {
        match self {
            RetrievalStageKind::Stage0ScipAnchor => Some("exact"),
            RetrievalStageKind::Stage1Lexical => Some("lexical_source"),
            RetrievalStageKind::Stage1bQdrantSemantic => Some("dense_anchor"),
            RetrievalStageKind::Stage2ScipExpand => Some("graph_neighbor"),
            RetrievalStageKind::Stage3RepoTextFallback => None,
        }
    }

    pub fn sidecar_latency_ms(self, elapsed_ms: u64) -> Option<u32> {
        match self {
            RetrievalStageKind::Stage0ScipAnchor
            | RetrievalStageKind::Stage1Lexical
            | RetrievalStageKind::Stage1bQdrantSemantic
            | RetrievalStageKind::Stage2ScipExpand => u32::try_from(elapsed_ms).ok(),
            RetrievalStageKind::Stage3RepoTextFallback => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// One planned stage with its local budget and candidate cap.
pub struct PlannedStage {
    pub kind: RetrievalStageKind,
    pub budget_ms: u64,
    pub top_k: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Query plan for one sidecar request.
///
/// Non-full degraded modes intentionally produce no stages so callers fail closed instead of
/// presenting partial sidecar coverage as complete retrieval.
pub struct RetrievalPlan {
    pub stages: Vec<PlannedStage>,
    pub total_budget_ms: u64,
    pub stop_marginal_gain_threshold: f32,
    pub stop_after_low_gain_streak: u32,
}

const DEFAULT_TOTAL_BUDGET_MS: u64 = 1_000;
const MARGINAL_GAIN_THRESHOLD: f32 = 0.05;
const LOW_GAIN_STREAK: u32 = 2;

/// Build the sidecar plan for a classified query and live retrieval mode.
///
/// Budget values are per-request guardrails, not SLA proof. Packet orchestration may divide a
/// larger packet budget across several calls before invoking this planner.
pub fn plan_query(features: &QueryFeatures, mode: RetrievalDegradedMode) -> RetrievalPlan {
    if mode != RetrievalDegradedMode::Full {
        return RetrievalPlan {
            stages: Vec::new(),
            total_budget_ms: 0,
            stop_marginal_gain_threshold: MARGINAL_GAIN_THRESHOLD,
            stop_after_low_gain_streak: LOW_GAIN_STREAK,
        };
    }

    let mut stages = Vec::new();
    let top_k = top_k_for_shape(features.shape);

    if mode.runs_scip_stages()
        && matches!(
            features.shape,
            QueryShape::SymbolLike | QueryShape::PathLike
        )
    {
        stages.push(PlannedStage {
            kind: RetrievalStageKind::Stage0ScipAnchor,
            budget_ms: stage0_budget_ms(features.shape),
            top_k: top_k.min(8),
        });
    }

    if mode.runs_lexical_stage() {
        stages.push(PlannedStage {
            kind: RetrievalStageKind::Stage1Lexical,
            budget_ms: stage1_budget_ms(features.shape),
            top_k,
        });
    }

    let qdrant_stage = if mode.runs_qdrant_stage() && features.shape != QueryShape::PathLike {
        let semantic_top_k = match features.shape {
            QueryShape::NaturalLanguage | QueryShape::Mixed => top_k.saturating_mul(2).min(40),
            _ => top_k,
        };
        Some(PlannedStage {
            kind: RetrievalStageKind::Stage1bQdrantSemantic,
            budget_ms: stage1b_budget_ms(features.shape),
            top_k: semantic_top_k,
        })
    } else {
        None
    };

    let scip_expand_stage = if mode.runs_scip_stages() {
        let stage2_top_k = match features.shape {
            QueryShape::NaturalLanguage => top_k.min(20),
            _ => top_k.min(16),
        };
        Some(PlannedStage {
            kind: RetrievalStageKind::Stage2ScipExpand,
            budget_ms: stage2_budget_ms(features.shape),
            top_k: stage2_top_k,
        })
    } else {
        None
    };

    if matches!(
        features.shape,
        QueryShape::NaturalLanguage | QueryShape::Mixed
    ) {
        stages.extend(qdrant_stage);
        stages.extend(scip_expand_stage);
    } else {
        stages.extend(scip_expand_stage);
        stages.extend(qdrant_stage);
    }

    let total_budget_ms = stages
        .iter()
        .map(|stage| stage.budget_ms)
        .sum::<u64>()
        .min(DEFAULT_TOTAL_BUDGET_MS);

    RetrievalPlan {
        stages,
        total_budget_ms,
        stop_marginal_gain_threshold: MARGINAL_GAIN_THRESHOLD,
        stop_after_low_gain_streak: LOW_GAIN_STREAK,
    }
}

fn top_k_for_shape(shape: QueryShape) -> usize {
    match shape {
        QueryShape::SymbolLike => 12,
        QueryShape::PathLike => 8,
        QueryShape::NaturalLanguage => 48,
        QueryShape::Mixed => 24,
    }
}

fn stage0_budget_ms(shape: QueryShape) -> u64 {
    match shape {
        QueryShape::SymbolLike | QueryShape::PathLike => 40,
        _ => 30,
    }
}

fn stage1_budget_ms(shape: QueryShape) -> u64 {
    match shape {
        QueryShape::NaturalLanguage | QueryShape::Mixed => 500,
        _ => 80,
    }
}

fn stage1b_budget_ms(shape: QueryShape) -> u64 {
    match shape {
        QueryShape::NaturalLanguage | QueryShape::Mixed => 250,
        QueryShape::SymbolLike => 120,
        QueryShape::PathLike => 0,
    }
    .max(80)
}

fn stage2_budget_ms(shape: QueryShape) -> u64 {
    match shape {
        QueryShape::SymbolLike | QueryShape::Mixed => 180,
        QueryShape::PathLike => 120,
        QueryShape::NaturalLanguage => 90,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query_features::classify_query;

    #[test]
    fn full_mode_includes_all_stages_for_symbol_query() {
        let features = classify_query("ExtensionService");
        let plan = plan_query(&features, RetrievalDegradedMode::Full);
        let kinds: Vec<_> = plan.stages.iter().map(|s| s.kind).collect();
        assert!(kinds.contains(&RetrievalStageKind::Stage0ScipAnchor));
        assert!(kinds.contains(&RetrievalStageKind::Stage1Lexical));
        assert!(kinds.contains(&RetrievalStageKind::Stage2ScipExpand));
        assert!(kinds.contains(&RetrievalStageKind::Stage1bQdrantSemantic));
        assert!(
            kinds
                .iter()
                .position(|kind| *kind == RetrievalStageKind::Stage2ScipExpand)
                < kinds
                    .iter()
                    .position(|kind| *kind == RetrievalStageKind::Stage1bQdrantSemantic)
        );
    }

    #[test]
    fn stage_kind_metadata_matches_sidecar_stage_contract() {
        let cases = [
            (RetrievalStageKind::Stage0ScipAnchor, Some("exact"), true),
            (
                RetrievalStageKind::Stage1Lexical,
                Some("lexical_source"),
                true,
            ),
            (
                RetrievalStageKind::Stage1bQdrantSemantic,
                Some("dense_anchor"),
                true,
            ),
            (
                RetrievalStageKind::Stage2ScipExpand,
                Some("graph_neighbor"),
                true,
            ),
            (RetrievalStageKind::Stage3RepoTextFallback, None, false),
        ];

        assert_eq!(cases.len(), 5);
        for (kind, expected_provenance, has_sidecar_latency) in cases {
            let serde_label = serde_json::to_value(kind).expect("stage kind serializes");

            assert_eq!(
                kind.label(),
                serde_label
                    .as_str()
                    .expect("stage kind serializes as a string")
            );
            assert_eq!(kind.provenance_label(), expected_provenance);
            assert_eq!(kind.sidecar_latency_ms(0), has_sidecar_latency.then_some(0));
            assert_eq!(
                kind.sidecar_latency_ms(u32::MAX.into()),
                has_sidecar_latency.then_some(u32::MAX)
            );
            assert_eq!(kind.sidecar_latency_ms(u64::from(u32::MAX) + 1), None);
        }
    }

    #[test]
    fn non_full_modes_have_no_product_stages() {
        let features = classify_query("ExtensionService");
        for mode in [
            RetrievalDegradedMode::NoScip,
            RetrievalDegradedMode::NoSemantic,
            RetrievalDegradedMode::LexicalOnly,
            RetrievalDegradedMode::Unavailable,
        ] {
            let plan = plan_query(&features, mode);
            assert!(plan.stages.is_empty(), "mode {mode:?} must fail closed");
        }
    }

    #[test]
    fn natural_language_plan_includes_scip_expand_stage() {
        let features = classify_query("how does request dispatch flow through interceptors");
        let plan = plan_query(&features, RetrievalDegradedMode::Full);
        let kinds: Vec<_> = plan.stages.iter().map(|s| s.kind).collect();
        assert!(!kinds.contains(&RetrievalStageKind::Stage0ScipAnchor));
        assert!(kinds.contains(&RetrievalStageKind::Stage2ScipExpand));
        assert!(kinds.contains(&RetrievalStageKind::Stage1bQdrantSemantic));
        assert!(
            kinds
                .iter()
                .position(|kind| *kind == RetrievalStageKind::Stage1bQdrantSemantic)
                < kinds
                    .iter()
                    .position(|kind| *kind == RetrievalStageKind::Stage2ScipExpand)
        );
    }

    #[test]
    fn mixed_prompt_plan_does_not_let_scip_anchor_starve_semantic_stage() {
        let features = classify_query("Explain how FooBar flows through request handling");
        let plan = plan_query(&features, RetrievalDegradedMode::Full);
        let kinds: Vec<_> = plan.stages.iter().map(|s| s.kind).collect();
        assert!(!kinds.contains(&RetrievalStageKind::Stage0ScipAnchor));
        assert!(kinds.contains(&RetrievalStageKind::Stage1Lexical));
        assert!(kinds.contains(&RetrievalStageKind::Stage2ScipExpand));
        assert!(kinds.contains(&RetrievalStageKind::Stage1bQdrantSemantic));
        assert!(
            kinds
                .iter()
                .position(|kind| *kind == RetrievalStageKind::Stage1bQdrantSemantic)
                < kinds
                    .iter()
                    .position(|kind| *kind == RetrievalStageKind::Stage2ScipExpand)
        );
    }
}
