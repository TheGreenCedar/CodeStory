use crate::mode::RetrievalDegradedMode;
use crate::query_features::{QueryFeatures, QueryShape};
use serde::{Deserialize, Serialize};

/// Staged retrieval lane (design doc staged retrieval).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetrievalStageKind {
    Stage0ScipAnchor,
    Stage1ZoektLexical,
    Stage1bQdrantSemantic,
    Stage2ScipExpand,
    Stage3RepoTextFallback,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlannedStage {
    pub kind: RetrievalStageKind,
    pub budget_ms: u64,
    pub top_k: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetrievalPlan {
    pub stages: Vec<PlannedStage>,
    pub total_budget_ms: u64,
    pub stop_marginal_gain_threshold: f32,
    pub stop_after_low_gain_streak: u32,
}

const DEFAULT_TOTAL_BUDGET_MS: u64 = 1_000;
const MARGINAL_GAIN_THRESHOLD: f32 = 0.05;
const LOW_GAIN_STREAK: u32 = 2;

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

    if mode.runs_zoekt_stage() {
        stages.push(PlannedStage {
            kind: RetrievalStageKind::Stage1ZoektLexical,
            budget_ms: stage1_budget_ms(features.shape),
            top_k,
        });
    }

    if mode.runs_qdrant_stage() && features.shape != QueryShape::PathLike {
        let semantic_top_k = match features.shape {
            QueryShape::NaturalLanguage | QueryShape::Mixed => top_k.saturating_mul(2).min(40),
            _ => top_k,
        };
        stages.push(PlannedStage {
            kind: RetrievalStageKind::Stage1bQdrantSemantic,
            budget_ms: stage1b_budget_ms(features.shape),
            top_k: semantic_top_k,
        });
    }

    if mode.runs_scip_stages() {
        let stage2_top_k = match features.shape {
            QueryShape::NaturalLanguage => top_k.min(20),
            _ => top_k.min(16),
        };
        stages.push(PlannedStage {
            kind: RetrievalStageKind::Stage2ScipExpand,
            budget_ms: stage2_budget_ms(features.shape),
            top_k: stage2_top_k,
        });
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
        QueryShape::NaturalLanguage | QueryShape::Mixed => 120,
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
        assert!(kinds.contains(&RetrievalStageKind::Stage1ZoektLexical));
        assert!(kinds.contains(&RetrievalStageKind::Stage1bQdrantSemantic));
        assert!(kinds.contains(&RetrievalStageKind::Stage2ScipExpand));
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
        assert!(kinds.contains(&RetrievalStageKind::Stage1bQdrantSemantic));
        assert!(kinds.contains(&RetrievalStageKind::Stage2ScipExpand));
    }

    #[test]
    fn mixed_prompt_plan_does_not_let_scip_anchor_starve_semantic_stage() {
        let features = classify_query("Explain how FooBar flows through request handling");
        let plan = plan_query(&features, RetrievalDegradedMode::Full);
        let kinds: Vec<_> = plan.stages.iter().map(|s| s.kind).collect();
        assert!(!kinds.contains(&RetrievalStageKind::Stage0ScipAnchor));
        assert!(kinds.contains(&RetrievalStageKind::Stage1ZoektLexical));
        assert!(kinds.contains(&RetrievalStageKind::Stage1bQdrantSemantic));
        assert!(kinds.contains(&RetrievalStageKind::Stage2ScipExpand));
    }
}
