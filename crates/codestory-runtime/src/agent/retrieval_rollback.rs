//! Rollback trigger checks for sidecar retrieval default rollout.
//!
//! Reads [`docs/architecture/retrieval-rollback.json`] and compares benchmark
//! summaries against baselines. On trigger, emits warnings only; sidecar
//! retrieval remains mandatory.

use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Debug, Deserialize)]
struct RollbackConfig {
    baseline_artifact_dir: String,
    comparison_artifact: String,
    consecutive_runs_required: u32,
    triggers: Vec<RollbackTrigger>,
}

#[derive(Debug, Deserialize)]
struct RollbackTrigger {
    id: String,
    metric: String,
    threshold: String,
}

#[derive(Debug, Default, Clone)]
pub(crate) struct RollbackCheckInput {
    pub p95_packet_wall_ms: Option<f64>,
    pub retrieval_p99_ms: Option<f64>,
    pub quality_pass_runs: Option<u32>,
    pub prior_quality_pass_runs: Option<u32>,
    pub sufficient_quality_mismatch_runs: Option<u32>,
    pub prior_sufficient_quality_mismatch_runs: Option<u32>,
    pub degraded_mode_rate: Option<f64>,
    pub holdout_claim_recall: Option<f64>,
    pub holdout_sufficient: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct RollbackWarning {
    pub trigger_id: String,
    pub message: String,
}

static DEGRADED_MODE_SAMPLES: Mutex<Vec<bool>> = Mutex::new(Vec::new());
const DEGRADED_SAMPLE_CAP: usize = 32;

/// Record whether the latest packet run was degraded; returns rolling degraded rate.
pub(crate) fn record_degraded_mode_sample(degraded: bool) -> f64 {
    let mut samples = DEGRADED_MODE_SAMPLES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    samples.push(degraded);
    if samples.len() > DEGRADED_SAMPLE_CAP {
        let overflow = samples.len() - DEGRADED_SAMPLE_CAP;
        samples.drain(0..overflow);
    }
    let degraded_count = samples.iter().filter(|sample| **sample).count();
    degraded_count as f64 / samples.len().max(1) as f64
}

#[cfg(test)]
pub(crate) fn reset_degraded_mode_samples_for_test() {
    let mut samples = DEGRADED_MODE_SAMPLES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    samples.clear();
}

fn rollback_config_path(repo_root: &Path) -> PathBuf {
    repo_root.join("docs/architecture/retrieval-rollback.json")
}

fn load_config(repo_root: &Path) -> Option<RollbackConfig> {
    let path = rollback_config_path(repo_root);
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

fn baseline_summary_path(repo_root: &Path, config: &RollbackConfig) -> PathBuf {
    repo_root
        .join(&config.baseline_artifact_dir)
        .join("local-real")
        .join(&config.comparison_artifact)
}

fn read_f64(value: &serde_json::Value) -> Option<f64> {
    value.as_f64().or_else(|| value.as_u64().map(|v| v as f64))
}

fn read_u32(value: &serde_json::Value) -> Option<u32> {
    value
        .as_u64()
        .and_then(|v| u32::try_from(v).ok())
        .or_else(|| value.as_i64().and_then(|v| u32::try_from(v).ok()))
}

fn baseline_metric(baseline: &serde_json::Value, metric: &str) -> Option<f64> {
    match metric {
        "p95_packet_wall_ms" => baseline
            .get("p95_packet_wall_ms")
            .or_else(|| baseline.get("packet_wall_p95_ms"))
            .and_then(read_f64),
        "retrieval_p99_ms" => baseline
            .get("retrieval_p99_ms")
            .or_else(|| baseline.get("retrieval_p99"))
            .and_then(read_f64),
        "quality_pass_runs" => baseline
            .get("quality_pass_runs")
            .and_then(read_u32)
            .map(f64::from),
        "sufficient_quality_mismatch_runs" => baseline
            .get("sufficient_quality_mismatch_runs")
            .and_then(read_u32)
            .map(f64::from),
        "degraded_mode_rate" => baseline.get("degraded_mode_rate").and_then(read_f64),
        "holdout_claim_recall" => baseline.get("holdout_claim_recall").and_then(read_f64),
        _ => None,
    }
}

fn percent_increase(current: f64, baseline: f64) -> f64 {
    if baseline <= 0.0 {
        return 0.0;
    }
    ((current - baseline) / baseline) * 100.0
}

fn evaluate_trigger(
    trigger: &RollbackTrigger,
    input: &RollbackCheckInput,
    baseline: &serde_json::Value,
) -> Option<RollbackWarning> {
    match trigger.id.as_str() {
        "p95_packet_wall_regression" => {
            let current = input.p95_packet_wall_ms?;
            let base = baseline_metric(baseline, &trigger.metric)?;
            let increase = percent_increase(current, base);
            if increase > 25.0 {
                Some(RollbackWarning {
                    trigger_id: trigger.id.clone(),
                    message: format!(
                        "p95 packet wall {current:.0}ms is +{increase:.1}% vs baseline {base:.0}ms (threshold {})",
                        trigger.threshold
                    ),
                })
            } else {
                None
            }
        }
        "retrieval_p99_regression" => {
            let current = input.retrieval_p99_ms?;
            let base = baseline_metric(baseline, &trigger.metric)?;
            let increase = percent_increase(current, base);
            if increase > 50.0 {
                Some(RollbackWarning {
                    trigger_id: trigger.id.clone(),
                    message: format!(
                        "retrieval p99 {current:.0}ms is +{increase:.1}% vs baseline {base:.0}ms (threshold {})",
                        trigger.threshold
                    ),
                })
            } else {
                None
            }
        }
        "quality_pass_runs_drop" => {
            let current = input.quality_pass_runs?;
            let prior = input.prior_quality_pass_runs?;
            if current < prior {
                Some(RollbackWarning {
                    trigger_id: trigger.id.clone(),
                    message: format!(
                        "quality_pass_runs dropped from {prior} to {current} (rollback config: >=1 repo regression)"
                    ),
                })
            } else {
                None
            }
        }
        "sufficient_quality_mismatch_increase" => {
            let current = input.sufficient_quality_mismatch_runs?;
            let prior = input.prior_sufficient_quality_mismatch_runs?;
            if current > prior {
                Some(RollbackWarning {
                    trigger_id: trigger.id.clone(),
                    message: format!(
                        "sufficient_quality_mismatch_runs increased from {prior} to {current}"
                    ),
                })
            } else {
                None
            }
        }
        "degraded_mode_rate" => {
            let current = input.degraded_mode_rate?;
            if current > 0.05 {
                Some(RollbackWarning {
                    trigger_id: trigger.id.clone(),
                    message: format!(
                        "degraded_mode_rate {:.1}% exceeds 5% (retrieval_mode != full)",
                        current * 100.0
                    ),
                })
            } else {
                None
            }
        }
        "holdout_claim_recall_floor" => {
            if !input.holdout_sufficient {
                return None;
            }
            let current = input.holdout_claim_recall?;
            if current < 0.5 {
                Some(RollbackWarning {
                    trigger_id: trigger.id.clone(),
                    message: format!(
                        "holdout claim recall {:.0}% below 50% floor while sufficient",
                        current * 100.0
                    ),
                })
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Evaluate rollback triggers; returns true when warnings fired.
pub(crate) fn check_and_log_rollback_warnings(
    repo_root: &Path,
    input: &RollbackCheckInput,
) -> bool {
    let Some(config) = load_config(repo_root) else {
        tracing::debug!("retrieval rollback config not found; skipping checks");
        return false;
    };

    let baseline_path = baseline_summary_path(repo_root, &config);
    let baseline = match std::fs::read_to_string(&baseline_path)
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
    {
        Some(value) => value,
        None => {
            tracing::debug!(
                path = %baseline_path.display(),
                "retrieval rollback baseline summary missing; skipping checks"
            );
            return false;
        }
    };

    let warnings: Vec<RollbackWarning> = config
        .triggers
        .iter()
        .filter_map(|trigger| evaluate_trigger(trigger, input, &baseline))
        .collect();

    if warnings.is_empty() {
        return false;
    }

    tracing::warn!(
        consecutive_runs_required = config.consecutive_runs_required,
        trigger_count = warnings.len(),
        "retrieval rollback triggers fired; sidecar retrieval remains mandatory"
    );
    for warning in warnings {
        tracing::warn!(trigger = %warning.trigger_id, "{}", warning.message);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_increase_computes_delta() {
        assert!((percent_increase(125.0, 100.0) - 25.0).abs() < f64::EPSILON);
    }

    #[test]
    fn p95_regression_trigger_fires_above_threshold() {
        let trigger = RollbackTrigger {
            id: "p95_packet_wall_regression".into(),
            metric: "p95_packet_wall_ms".into(),
            threshold: "+25%".into(),
        };
        let baseline = serde_json::json!({ "p95_packet_wall_ms": 1000.0 });
        let warning = evaluate_trigger(
            &trigger,
            &RollbackCheckInput {
                p95_packet_wall_ms: Some(1300.0),
                ..Default::default()
            },
            &baseline,
        );
        assert!(warning.is_some());
    }

    #[test]
    fn rollback_drill_warns_without_setting_legacy_env() {
        let repo = std::env::current_dir().expect("cwd");
        // SAFETY: test-only env cleanup before checking rollback behavior.
        unsafe {
            std::env::remove_var("CODESTORY_RETRIEVAL");
        }
        let fired = check_and_log_rollback_warnings(
            &repo,
            &RollbackCheckInput {
                p95_packet_wall_ms: Some(10_000.0),
                ..Default::default()
            },
        );
        if fired {
            assert_ne!(
                std::env::var("CODESTORY_RETRIEVAL").ok().as_deref(),
                Some("0")
            );
        }
    }

    #[test]
    fn degraded_mode_rate_trigger_fires_above_five_percent() {
        reset_degraded_mode_samples_for_test();
        let rate = record_degraded_mode_sample(true);
        assert!(rate > 0.05);
        let trigger = RollbackTrigger {
            id: "degraded_mode_rate".into(),
            metric: "degraded_mode_rate".into(),
            threshold: ">5%".into(),
        };
        let baseline = serde_json::json!({});
        let warning = evaluate_trigger(
            &trigger,
            &RollbackCheckInput {
                degraded_mode_rate: Some(rate),
                ..Default::default()
            },
            &baseline,
        );
        assert!(warning.is_some());
    }
}
