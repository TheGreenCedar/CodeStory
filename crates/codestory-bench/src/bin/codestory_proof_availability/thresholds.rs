use super::contracts::{
    ActivationDecisionV1, ActivationOutcomeV1, CohortObservationV1, CorpusV1,
    DerivedObservationsV1, FailedGateV1, GateFailureDetailV1, HardGateObservationsV1,
    MaterializationFreshnessV1, McpRevisionV1, ProductDispositionKindV1, QualificationGateKindV1,
    QualificationSummaryV1, RatioObservationV1, RoleThresholdsV1, SourceDependencyEvidenceV1,
    ThresholdsV1, TransportErrorV1, TransportEvidenceV1, TransportP95ObservationV1,
    WilsonObservationV1,
};
use anyhow::{Result, bail};
use std::collections::BTreeMap;

const WILSON_Z: f64 = 1.959_963_984_540_054;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WilsonScoreInterval {
    pub numerator: u64,
    pub denominator: u64,
    pub lower: f64,
    pub upper: f64,
    pub lower_milli: u16,
}

pub fn wilson_score_interval(
    numerator: u64,
    denominator: u64,
    z: f64,
) -> Result<WilsonScoreInterval> {
    if denominator == 0 || numerator > denominator || !z.is_finite() || z <= 0.0 {
        bail!("proof_availability_wilson_input_invalid")
    }
    let n = denominator as f64;
    let p = numerator as f64 / n;
    let z_squared = z * z;
    let adjustment = 1.0 + z_squared / n;
    let center = (p + z_squared / (2.0 * n)) / adjustment;
    let half_width = z * (p * (1.0 - p) / n + z_squared / (4.0 * n * n)).sqrt() / adjustment;
    let lower = (center - half_width).clamp(0.0, 1.0);
    let upper = (center + half_width).clamp(0.0, 1.0);
    // This scaled value is presentation only. Gate comparisons use `lower`
    // directly so rounding can never turn a statistical miss into a pass.
    let lower_milli = (lower * 1_000.0).floor().clamp(0.0, 1_000.0) as u16;
    Ok(WilsonScoreInterval {
        numerator,
        denominator,
        lower,
        upper,
        lower_milli,
    })
}

#[derive(Debug, Clone)]
struct Observations {
    full_proofs: u64,
    full_proofs_by_cohort: BTreeMap<String, u64>,
    positive_requests: u64,
    positive_requests_by_cohort: BTreeMap<String, u64>,
    exact_positive_steps: u64,
    positive_steps: u64,
    full_or_useful: u64,
    incomplete: u64,
    actionable_incomplete: u64,
    positive_step_recall_milli: u16,
    full_or_useful_partial_milli: u16,
    actionable_incomplete_gap_milli: u16,
    unknown_warm_p95_ms: u64,
    transport_p95_ns: [u64; 4],
    complete_response_p95_bytes: u64,
    unknown_response_p95_bytes: u64,
    maximum_response_bytes: u64,
    false_contract_proven: u64,
    non_exact_authoritative_receipts: u64,
    certified_absence: u64,
    unclassified_positive_steps: u64,
    incomplete_provenance: u64,
    invalid_results: u64,
    over_cap_results: u64,
    transport_errors: u64,
    product_disposition_mismatches: u64,
}

pub(crate) fn derive_observations(
    summary: &QualificationSummaryV1,
    thresholds: &ThresholdsV1,
) -> Result<DerivedObservationsV1> {
    let observed = observations(summary)?;
    let full_wilson = wilson_score_interval(
        observed.full_proofs,
        observed.positive_requests,
        thresholds.wilson_z,
    )?;
    let cohorts = observed
        .full_proofs_by_cohort
        .iter()
        .map(|(repository_id, numerator)| {
            let denominator = *observed
                .positive_requests_by_cohort
                .get(repository_id)
                .ok_or_else(|| anyhow::anyhow!("proof_availability_cohort_missing"))?;
            let wilson = wilson_score_interval(*numerator, denominator, thresholds.wilson_z)?;
            Ok(CohortObservationV1 {
                repository_id: repository_id.clone(),
                full_proofs: ratio_observation(*numerator, denominator)?,
                wilson: wilson_observation(wilson),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(DerivedObservationsV1 {
        full_proofs: ratio_observation(observed.full_proofs, observed.positive_requests)?,
        full_proof_wilson: wilson_observation(full_wilson),
        cohorts,
        positive_step_recall: ratio_observation(
            observed.exact_positive_steps,
            observed.positive_steps,
        )?,
        full_or_useful_partial: ratio_observation(
            observed.full_or_useful,
            observed.positive_requests,
        )?,
        actionable_incomplete_gap: if observed.incomplete == 0 {
            RatioObservationV1 {
                numerator: 0,
                denominator: 0,
                milli: 1_000,
            }
        } else {
            ratio_observation(observed.actionable_incomplete, observed.incomplete)?
        },
        unknown_warm_p95_ms: observed.unknown_warm_p95_ms,
        transport_p95: [
            McpRevisionV1::V2024_11_05,
            McpRevisionV1::V2025_03_26,
            McpRevisionV1::V2025_06_18,
            McpRevisionV1::V2025_11_25,
        ]
        .into_iter()
        .zip(observed.transport_p95_ns)
        .map(|(revision, elapsed_ns)| TransportP95ObservationV1 {
            revision,
            elapsed_ns,
        })
        .collect(),
        complete_response_p95_bytes: observed.complete_response_p95_bytes,
        unknown_response_p95_bytes: observed.unknown_response_p95_bytes,
        maximum_response_bytes: observed.maximum_response_bytes,
        hard_gates: HardGateObservationsV1 {
            false_contract_proven: observed.false_contract_proven,
            non_exact_authoritative_receipts: observed.non_exact_authoritative_receipts,
            certified_absence: observed.certified_absence,
            unclassified_positive_steps: observed.unclassified_positive_steps,
            incomplete_provenance: observed.incomplete_provenance,
            invalid_results: observed.invalid_results,
            over_cap_results: observed.over_cap_results,
            transport_errors: observed.transport_errors,
            product_disposition_mismatches: observed.product_disposition_mismatches,
        },
    })
}

fn ratio_observation(numerator: u64, denominator: u64) -> Result<RatioObservationV1> {
    Ok(RatioObservationV1 {
        numerator,
        denominator,
        milli: ratio_milli(numerator, denominator)?,
    })
}

fn wilson_observation(value: WilsonScoreInterval) -> WilsonObservationV1 {
    WilsonObservationV1 {
        numerator: value.numerator,
        denominator: value.denominator,
        lower: value.lower,
        upper: value.upper,
        lower_milli: value.lower_milli,
    }
}

pub fn evaluate_activation_decision(
    summary: &QualificationSummaryV1,
    corpus: &CorpusV1,
    thresholds: &ThresholdsV1,
    source_dependency: Option<&SourceDependencyEvidenceV1>,
) -> Result<ActivationDecisionV1> {
    summary.validate_against_inputs(corpus, thresholds)?;
    let observations = observations(summary)?;
    decision_from_observations(&observations, thresholds, source_dependency)
}

fn decision_from_observations(
    observations: &Observations,
    thresholds: &ThresholdsV1,
    source_dependency: Option<&SourceDependencyEvidenceV1>,
) -> Result<ActivationDecisionV1> {
    let mut failed_gates = hard_gate_failures(observations, thresholds);
    let hard_failed = !failed_gates.is_empty();
    let automatic = role_failures(
        "automatic",
        QualificationGateKindV1::AutomaticThreshold,
        observations,
        &thresholds.automatic,
        CohortRule::Every,
        thresholds.wilson_z,
    )?;
    let stable = role_failures(
        "stable",
        QualificationGateKindV1::StableThreshold,
        observations,
        &thresholds.stable_explicit,
        CohortRule::Every,
        thresholds.wilson_z,
    )?;
    let experimental = role_failures(
        "experimental",
        QualificationGateKindV1::ExperimentalUsefulness,
        observations,
        &thresholds.experimental,
        CohortRule::AtLeastOne,
        thresholds.wilson_z,
    )?;
    let automatic_met = automatic.role_met;
    failed_gates.extend(automatic.failures);
    failed_gates.extend(stable.failures);
    failed_gates.extend(experimental.failures);

    let outcome = if let Some(evidence) = source_dependency {
        failed_gates.insert(
            0,
            FailedGateV1 {
                gate_id: "integration.source_dependency".to_owned(),
                kind: QualificationGateKindV1::IntegrationDependency,
                detail: GateFailureDetailV1::SourceDependency {
                    evidence: Box::new(evidence.clone()),
                },
            },
        );
        ActivationOutcomeV1::DelayFullV3Cut
    } else if hard_failed {
        ActivationOutcomeV1::KeepProofDark
    } else if stable.role_met {
        ActivationOutcomeV1::PublicExactVerifier
    } else if experimental.role_met {
        ActivationOutcomeV1::ExperimentalManualVerifier
    } else {
        ActivationOutcomeV1::KeepProofDark
    };
    let decision = ActivationDecisionV1 {
        automatic_thresholds_met: if matches!(outcome, ActivationOutcomeV1::DelayFullV3Cut) {
            None
        } else {
            Some(automatic_met)
        },
        outcome,
        failed_gates,
    };
    decision.validate()?;
    Ok(decision)
}

fn observations(summary: &QualificationSummaryV1) -> Result<Observations> {
    let mut full_proofs = 0u64;
    let mut full_proofs_by_cohort = summary
        .environment
        .projects
        .iter()
        .map(|project| (project.repository_id.clone(), 0u64))
        .collect::<BTreeMap<_, _>>();
    let mut positive_requests_by_cohort = summary
        .environment
        .projects
        .iter()
        .map(|project| (project.repository_id.clone(), 0u64))
        .collect::<BTreeMap<_, _>>();
    let mut exact_steps = 0u64;
    let mut full_or_useful = 0u64;
    let mut incomplete = 0u64;
    let mut actionable_incomplete = 0u64;
    let mut unknown_warm = Vec::new();
    let mut transport_elapsed = [Vec::new(), Vec::new(), Vec::new(), Vec::new()];
    let mut complete_bytes = Vec::new();
    let mut unknown_bytes = Vec::new();
    let mut maximum_response_bytes = 0u64;
    let mut false_contract_proven = 0u64;
    let mut non_exact_authoritative_receipts = 0u64;
    let mut certified_absence = 0u64;
    let mut invalid_results = 0u64;
    let mut over_cap_results = 0u64;
    let mut transport_errors = 0u64;
    let mut product_disposition_mismatches = 0u64;

    for case in &summary.cases {
        *positive_requests_by_cohort
            .get_mut(&case.repository_id)
            .ok_or_else(|| anyhow::anyhow!("proof_availability_cohort_missing"))? += 1;
        let metrics = case.receipt_metrics()?;
        let facts = case.evaluable_facts()?;
        exact_steps = exact_steps
            .checked_add(u64::from(metrics.exact_oracle_step_count))
            .ok_or_else(|| anyhow::anyhow!("proof_availability_metric_overflow"))?;
        non_exact_authoritative_receipts = non_exact_authoritative_receipts
            .checked_add(
                metrics
                    .authoritative_receipt_count
                    .saturating_sub(metrics.authoritative_exact_receipt_count),
            )
            .ok_or_else(|| anyhow::anyhow!("proof_availability_metric_overflow"))?;
        if !facts.product_disposition_matches_evidence {
            product_disposition_mismatches += 1;
        }
        if facts.false_contract_proven {
            false_contract_proven += 1;
        }
        false_contract_proven = false_contract_proven
            .checked_add(u64::try_from(
                case.negative_mutations
                    .iter()
                    .filter(|mutation| mutation.contract_proven)
                    .count(),
            )?)
            .ok_or_else(|| anyhow::anyhow!("proof_availability_metric_overflow"))?;

        let is_full = facts.contract_proven_supported
            && matches!(
                case.product_disposition.kind,
                ProductDispositionKindV1::ContractProven
            );
        if is_full {
            full_proofs += 1;
            *full_proofs_by_cohort
                .get_mut(&case.repository_id)
                .ok_or_else(|| anyhow::anyhow!("proof_availability_cohort_missing"))? += 1;
            full_or_useful += 1;
        } else {
            incomplete += 1;
            let actionable = case.actionable_exact_gap.is_some();
            actionable_incomplete += u64::from(actionable);
            if metrics.proven_prefix_length > 0 && actionable {
                full_or_useful += 1;
            }
        }
        match case.product_disposition.kind {
            ProductDispositionKindV1::Unknown => unknown_warm.push(case.warm_end_to_end_ms),
            ProductDispositionKindV1::CertifiedAbsence => certified_absence += 1,
            ProductDispositionKindV1::Invalid => invalid_results += 1,
            ProductDispositionKindV1::ContractProven => {}
        }
        if case.complete_projection_bytes > 0 {
            maximum_response_bytes = maximum_response_bytes.max(case.complete_projection_bytes);
        }
        match &case.transport {
            TransportEvidenceV1::Measurements { measurements } => {
                let mut case_max = 0u64;
                for (index, measurement) in measurements.measurements.iter().enumerate() {
                    transport_elapsed[index].push(measurement.elapsed_ns);
                    maximum_response_bytes = maximum_response_bytes.max(measurement.actual_bytes);
                    case_max = case_max.max(measurement.actual_bytes);
                }
                if matches!(
                    case.product_disposition.kind,
                    ProductDispositionKindV1::Unknown
                ) {
                    unknown_bytes.push(case_max);
                } else if matches!(
                    case.product_disposition.kind,
                    ProductDispositionKindV1::ContractProven
                ) {
                    complete_bytes.push(case_max);
                }
            }
            TransportEvidenceV1::Error { error } => {
                transport_errors += 1;
                if let TransportErrorV1::ResultExceedsBudget {
                    maximum_bytes,
                    actual_bytes,
                } = error
                {
                    over_cap_results += 1;
                    maximum_response_bytes = maximum_response_bytes
                        .max(*maximum_bytes)
                        .max(*actual_bytes);
                }
            }
        }
    }
    let incomplete_provenance = u64::try_from(
        summary
            .environment
            .projects
            .iter()
            .filter(|project| !matches!(project.freshness, MaterializationFreshnessV1::Fresh))
            .count(),
    )?;
    Ok(Observations {
        full_proofs,
        full_proofs_by_cohort,
        positive_requests: u64::try_from(summary.cases.len())?,
        positive_requests_by_cohort,
        exact_positive_steps: exact_steps,
        positive_steps: u64::from(summary.failure_funnel.attempted_positive_steps),
        full_or_useful,
        incomplete,
        actionable_incomplete,
        positive_step_recall_milli: ratio_milli(exact_steps, 312)?,
        full_or_useful_partial_milli: ratio_milli(full_or_useful, 120)?,
        actionable_incomplete_gap_milli: if incomplete == 0 {
            1_000
        } else {
            ratio_milli(actionable_incomplete, incomplete)?
        },
        unknown_warm_p95_ms: nearest_rank_p95(&mut unknown_warm),
        transport_p95_ns: transport_elapsed.map(|mut values| nearest_rank_p95(&mut values)),
        complete_response_p95_bytes: nearest_rank_p95(&mut complete_bytes),
        unknown_response_p95_bytes: nearest_rank_p95(&mut unknown_bytes),
        maximum_response_bytes,
        false_contract_proven,
        non_exact_authoritative_receipts,
        certified_absence,
        unclassified_positive_steps: u64::from(summary.failure_funnel.unclassified_positive_steps),
        incomplete_provenance,
        invalid_results,
        over_cap_results,
        transport_errors,
        product_disposition_mismatches,
    })
}

fn ratio_milli(numerator: u64, denominator: u64) -> Result<u16> {
    if denominator == 0 || numerator > denominator {
        bail!("proof_availability_ratio_invalid")
    }
    let scaled = numerator
        .checked_mul(1_000)
        .and_then(|value| value.checked_add(denominator / 2))
        .ok_or_else(|| anyhow::anyhow!("proof_availability_ratio_overflow"))?;
    Ok(u16::try_from(scaled / denominator)?)
}

fn nearest_rank_p95(values: &mut [u64]) -> u64 {
    if values.is_empty() {
        return 0;
    }
    values.sort_unstable();
    let rank = (95 * values.len()).div_ceil(100);
    values[rank.saturating_sub(1)]
}

fn count_failure(
    gate_id: impl Into<String>,
    kind: QualificationGateKindV1,
    observed: u64,
    required: u64,
) -> FailedGateV1 {
    FailedGateV1 {
        gate_id: gate_id.into(),
        kind,
        detail: GateFailureDetailV1::Count {
            observed: u128::from(observed),
            required: u128::from(required),
        },
    }
}

fn hard_gate_failures(observations: &Observations, thresholds: &ThresholdsV1) -> Vec<FailedGateV1> {
    let hard = &thresholds.hard_gates;
    let checks = [
        (
            "hard.false_contract_proven",
            QualificationGateKindV1::FalseContractProven,
            observations.false_contract_proven,
            u64::from(hard.maximum_false_contract_proven),
        ),
        (
            "hard.authoritative_receipt_mismatch",
            QualificationGateKindV1::ReceiptMismatch,
            observations.non_exact_authoritative_receipts,
            0,
        ),
        (
            "hard.production_certified_absence",
            QualificationGateKindV1::CertifiedAbsence,
            observations.certified_absence,
            u64::from(hard.maximum_certified_absence),
        ),
        (
            "hard.unclassified_positive_steps",
            QualificationGateKindV1::FailureFunnel,
            observations.unclassified_positive_steps,
            0,
        ),
        (
            "hard.incomplete_provenance",
            QualificationGateKindV1::Provenance,
            observations.incomplete_provenance,
            0,
        ),
        (
            "hard.invalid_results",
            QualificationGateKindV1::ResponseSize,
            observations.invalid_results,
            u64::from(hard.maximum_invalid_results),
        ),
        (
            "hard.over_cap_results",
            QualificationGateKindV1::ResponseSize,
            observations.over_cap_results,
            u64::from(hard.maximum_over_cap_results),
        ),
        (
            "hard.transport_errors",
            QualificationGateKindV1::ResponseSize,
            observations.transport_errors,
            u64::from(hard.maximum_transport_errors),
        ),
        (
            "hard.maximum_response_bytes",
            QualificationGateKindV1::ResponseSize,
            observations.maximum_response_bytes,
            hard.maximum_proof_bytes,
        ),
        (
            "hard.product_disposition_mismatch",
            QualificationGateKindV1::ProductDispositionMismatch,
            observations.product_disposition_mismatches,
            0,
        ),
    ];
    checks
        .into_iter()
        .filter(|(_, _, observed, maximum)| observed > maximum)
        .map(|(id, kind, observed, maximum)| count_failure(id, kind, observed, maximum))
        .collect()
}

#[derive(Debug, Clone, Copy)]
enum CohortRule {
    Every,
    AtLeastOne,
}

struct RoleEvaluation {
    role_met: bool,
    failures: Vec<FailedGateV1>,
}

fn role_failures(
    prefix: &str,
    kind: QualificationGateKindV1,
    observations: &Observations,
    thresholds: &RoleThresholdsV1,
    cohort_rule: CohortRule,
    wilson_z: f64,
) -> Result<RoleEvaluation> {
    let mut failures = Vec::new();
    let overall = wilson_score_interval(observations.full_proofs, 120, wilson_z)?;
    minimum(
        &mut failures,
        format!("{prefix}.full_proofs.count"),
        kind.clone(),
        observations.full_proofs,
        u64::from(thresholds.minimum_full_proofs),
    );
    if overall.lower < f64::from(thresholds.minimum_full_proof_wilson_lower_milli) / 1_000.0 {
        failures.push(count_failure(
            format!("{prefix}.full_proofs.wilson_lower_milli"),
            kind.clone(),
            u64::from(overall.lower_milli),
            u64::from(thresholds.minimum_full_proof_wilson_lower_milli),
        ));
    }

    let mut passing_cohorts = 0usize;
    for (repository_id, observed) in &observations.full_proofs_by_cohort {
        let interval = wilson_score_interval(*observed, 30, wilson_z)?;
        let count_pass = *observed >= u64::from(thresholds.minimum_full_proofs_per_cohort);
        let wilson_pass =
            interval.lower >= f64::from(thresholds.minimum_cohort_wilson_lower_milli) / 1_000.0;
        if count_pass && wilson_pass {
            passing_cohorts += 1;
        }
        if !count_pass {
            failures.push(FailedGateV1 {
                gate_id: format!("{prefix}.cohort.{repository_id}.count"),
                kind: kind.clone(),
                detail: GateFailureDetailV1::Cohort {
                    repository_id: repository_id.clone(),
                    observed: u128::from(*observed),
                    required: u128::from(thresholds.minimum_full_proofs_per_cohort),
                },
            });
        }
        if !wilson_pass {
            failures.push(FailedGateV1 {
                gate_id: format!("{prefix}.cohort.{repository_id}.wilson_lower_milli"),
                kind: kind.clone(),
                detail: GateFailureDetailV1::Cohort {
                    repository_id: repository_id.clone(),
                    observed: u128::from(interval.lower_milli),
                    required: u128::from(thresholds.minimum_cohort_wilson_lower_milli),
                },
            });
        }
    }
    let cohort_met = match cohort_rule {
        CohortRule::Every => passing_cohorts == observations.full_proofs_by_cohort.len(),
        CohortRule::AtLeastOne => passing_cohorts > 0,
    };
    if !cohort_met {
        failures.push(count_failure(
            format!("{prefix}.cohort.requirement"),
            kind.clone(),
            u64::try_from(passing_cohorts)?,
            match cohort_rule {
                CohortRule::Every => u64::try_from(observations.full_proofs_by_cohort.len())?,
                CohortRule::AtLeastOne => 1,
            },
        ));
    }

    minimum(
        &mut failures,
        format!("{prefix}.positive_step_recall_milli"),
        kind.clone(),
        u64::from(observations.positive_step_recall_milli),
        u64::from(thresholds.minimum_positive_step_recall_milli),
    );
    minimum(
        &mut failures,
        format!("{prefix}.full_or_useful_partial_milli"),
        kind.clone(),
        u64::from(observations.full_or_useful_partial_milli),
        u64::from(thresholds.minimum_full_or_useful_partial_milli),
    );
    minimum(
        &mut failures,
        format!("{prefix}.actionable_incomplete_gap_milli"),
        kind.clone(),
        u64::from(observations.actionable_incomplete_gap_milli),
        u64::from(thresholds.minimum_actionable_exact_gap_milli),
    );
    maximum(
        &mut failures,
        format!("{prefix}.unknown_warm_p95_ms"),
        kind.clone(),
        observations.unknown_warm_p95_ms,
        thresholds.maximum_unknown_p95_ms,
    );
    maximum(
        &mut failures,
        format!("{prefix}.complete_response_p95_bytes"),
        kind.clone(),
        observations.complete_response_p95_bytes,
        thresholds.maximum_complete_response_p95_bytes,
    );
    maximum(
        &mut failures,
        format!("{prefix}.unknown_response_p95_bytes"),
        kind.clone(),
        observations.unknown_response_p95_bytes,
        thresholds.maximum_unknown_response_p95_bytes,
    );
    maximum(
        &mut failures,
        format!("{prefix}.maximum_response_bytes"),
        kind.clone(),
        observations.maximum_response_bytes,
        thresholds.maximum_response_bytes,
    );
    let maximum_transport_ns = thresholds
        .maximum_transport_p95_ms
        .checked_mul(1_000_000)
        .ok_or_else(|| anyhow::anyhow!("proof_availability_transport_threshold_overflow"))?;
    for (index, revision) in ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"]
        .iter()
        .enumerate()
    {
        maximum(
            &mut failures,
            format!("{prefix}.transport.{revision}.p95_ns"),
            kind.clone(),
            observations.transport_p95_ns[index],
            maximum_transport_ns,
        );
    }
    let non_cohort_failure = failures
        .iter()
        .any(|failure| !failure.gate_id.starts_with(&format!("{prefix}.cohort.")));
    let role_met = cohort_met
        && !non_cohort_failure
        && (matches!(cohort_rule, CohortRule::AtLeastOne) || failures.is_empty());
    Ok(RoleEvaluation { role_met, failures })
}

fn minimum(
    failures: &mut Vec<FailedGateV1>,
    id: String,
    kind: QualificationGateKindV1,
    observed: u64,
    required: u64,
) {
    if observed < required {
        failures.push(count_failure(id, kind, observed, required));
    }
}
fn maximum(
    failures: &mut Vec<FailedGateV1>,
    id: String,
    kind: QualificationGateKindV1,
    observed: u64,
    required: u64,
) {
    if observed > required {
        failures.push(count_failure(id, kind, observed, required));
    }
}

#[cfg(test)]
mod tests {
    use super::super::contracts::{
        IntegrationDependencyTestKindV1, IntegrationDependencyTestStatusV1,
        IntegrationDependencyTestV1, OracleSourceRangeV1, SourceDependencyCoordinateV1,
        SourceDependencyKindV1, canonical_thresholds_sha256, results_evidence_sha256_from_json,
    };
    use super::*;
    use sha2::{Digest, Sha256};

    #[allow(clippy::duplicate_mod)] // Reuse the accepted closed 120-case fixture verbatim.
    mod accepted_fixture {
        include!("../../../tests/proof_availability_contracts.rs");

        pub(super) fn values() -> (serde_json::Value, serde_json::Value, serde_json::Value) {
            (report(), corpus(), thresholds())
        }
    }

    fn observed(full: u64, cohorts: [u64; 4]) -> Observations {
        Observations {
            full_proofs: full,
            full_proofs_by_cohort: ["a", "b", "c", "d"]
                .into_iter()
                .zip(cohorts)
                .map(|(id, count)| (id.to_owned(), count))
                .collect(),
            positive_requests: 120,
            positive_requests_by_cohort: ["a", "b", "c", "d"]
                .into_iter()
                .map(|id| (id.to_owned(), 30))
                .collect(),
            exact_positive_steps: 312,
            positive_steps: 312,
            full_or_useful: 120,
            incomplete: 120u64.saturating_sub(full),
            actionable_incomplete: 120u64.saturating_sub(full),
            positive_step_recall_milli: 1_000,
            full_or_useful_partial_milli: 1_000,
            actionable_incomplete_gap_milli: 1_000,
            unknown_warm_p95_ms: 1,
            transport_p95_ns: [1; 4],
            complete_response_p95_bytes: 128,
            unknown_response_p95_bytes: 128,
            maximum_response_bytes: 128,
            false_contract_proven: 0,
            non_exact_authoritative_receipts: 0,
            certified_absence: 0,
            unclassified_positive_steps: 0,
            incomplete_provenance: 0,
            invalid_results: 0,
            over_cap_results: 0,
            transport_errors: 0,
            product_disposition_mismatches: 0,
        }
    }

    fn refresh_results_digest(report: &mut serde_json::Value) {
        let digest = results_evidence_sha256_from_json(report).expect("recomputed results digest");
        report["provenance"]["results_sha256"] = serde_json::json!(digest);
    }
    fn threshold_role(full: u16, cohort: u16, lower: u16, cohort_lower: u16) -> RoleThresholdsV1 {
        RoleThresholdsV1 {
            minimum_full_proofs: full,
            minimum_full_proofs_per_cohort: cohort,
            minimum_full_proof_wilson_lower_milli: lower,
            minimum_cohort_wilson_lower_milli: cohort_lower,
            minimum_positive_step_recall_milli: 0,
            minimum_full_or_useful_partial_milli: 0,
            minimum_actionable_exact_gap_milli: 0,
            maximum_unknown_p95_ms: u64::MAX,
            maximum_transport_p95_ms: u64::MAX / 1_000_000,
            maximum_complete_response_p95_bytes: u64::MAX,
            maximum_unknown_response_p95_bytes: u64::MAX,
            maximum_response_bytes: u64::MAX,
        }
    }
    fn frozen() -> ThresholdsV1 {
        serde_json::from_str(include_str!(
            "../../../../../benchmarks/proof-availability/thresholds-v1.json"
        ))
        .expect("frozen thresholds")
    }

    #[test]
    fn wilson_boundaries_preserve_raw_counts() {
        for (n, d, expected) in [
            (96, 120, 719),
            (95, 120, 710),
            (60, 120, 411),
            (59, 120, 403),
            (24, 120, 138),
            (23, 120, 131),
            (21, 30, 521),
            (20, 30, 487),
            (12, 30, 245),
            (11, 30, 218),
        ] {
            let interval = wilson_score_interval(n, d, WILSON_Z).expect("interval");
            assert_eq!(
                (
                    interval.numerator,
                    interval.denominator,
                    interval.lower_milli
                ),
                (n, d, expected)
            );
        }
        let automatic = wilson_score_interval(96, 120, WILSON_Z).expect("automatic boundary");
        assert!((automatic.lower - 0.719_633_264_937_180_5).abs() < 1e-15);
        let experimental = wilson_score_interval(24, 120, WILSON_Z).expect("experimental boundary");
        assert!((experimental.lower - 0.138_244_764_788_402_56).abs() < 1e-15);
    }

    #[test]
    fn point_and_cohort_boundaries_are_exact() {
        for (count, required, lower, pass) in [
            (95, 96, 720, false),
            (96, 96, 720, false),
            (97, 96, 720, true),
            (59, 60, 410, false),
            (60, 60, 410, true),
            (23, 24, 140, false),
            (24, 24, 140, false),
            (25, 24, 140, true),
        ] {
            let result = role_failures(
                "role",
                QualificationGateKindV1::ExperimentalUsefulness,
                &observed(count, [count / 4; 4]),
                &threshold_role(required, 0, lower, 0),
                CohortRule::Every,
                WILSON_Z,
            )
            .expect("role");
            assert_eq!(result.role_met, pass, "{count}/{required}");
        }
        for (count, required, lower, pass) in [
            (20, 21, 500, false),
            (21, 21, 500, true),
            (11, 12, 240, false),
            (12, 12, 240, true),
        ] {
            let result = role_failures(
                "role",
                QualificationGateKindV1::StableThreshold,
                &observed(count * 4, [count; 4]),
                &threshold_role(0, required, 0, lower),
                CohortRule::Every,
                WILSON_Z,
            )
            .expect("cohort role");
            assert_eq!(result.role_met, pass, "cohort {count}/{required}");
        }
    }

    #[test]
    fn cohorts_cannot_be_averaged_and_experimental_needs_one() {
        let automatic = role_failures(
            "automatic",
            QualificationGateKindV1::AutomaticThreshold,
            &observed(96, [20, 25, 25, 26]),
            &threshold_role(96, 21, 720, 500),
            CohortRule::Every,
            WILSON_Z,
        )
        .expect("automatic");
        assert!(!automatic.role_met);
        let stable = role_failures(
            "stable",
            QualificationGateKindV1::StableThreshold,
            &observed(60, [11, 16, 16, 17]),
            &threshold_role(60, 12, 410, 240),
            CohortRule::Every,
            WILSON_Z,
        )
        .expect("stable");
        assert!(!stable.role_met);
        let none = role_failures(
            "experimental",
            QualificationGateKindV1::ExperimentalUsefulness,
            &observed(44, [11; 4]),
            &threshold_role(24, 12, 140, 0),
            CohortRule::AtLeastOne,
            WILSON_Z,
        )
        .expect("none");
        assert!(!none.role_met);
        let one = role_failures(
            "experimental",
            QualificationGateKindV1::ExperimentalUsefulness,
            &observed(45, [12, 11, 11, 11]),
            &threshold_role(24, 12, 140, 0),
            CohortRule::AtLeastOne,
            WILSON_Z,
        )
        .expect("one");
        assert!(one.role_met);
        assert_eq!(
            one.failures
                .iter()
                .filter(|failure| failure.gate_id.ends_with(".count"))
                .count(),
            3
        );
    }

    #[test]
    fn hard_failures_are_complete_and_canonically_ordered() {
        let mut value = observed(0, [0; 4]);
        value.false_contract_proven = 1;
        value.non_exact_authoritative_receipts = 1;
        value.certified_absence = 1;
        value.unclassified_positive_steps = 1;
        value.incomplete_provenance = 1;
        value.invalid_results = 1;
        value.over_cap_results = 1;
        value.transport_errors = 1;
        value.maximum_response_bytes = 65_537;
        value.product_disposition_mismatches = 1;
        let ids = hard_gate_failures(&value, &frozen())
            .into_iter()
            .map(|failure| failure.gate_id)
            .collect::<Vec<_>>();
        assert_eq!(
            ids,
            [
                "hard.false_contract_proven",
                "hard.authoritative_receipt_mismatch",
                "hard.production_certified_absence",
                "hard.unclassified_positive_steps",
                "hard.incomplete_provenance",
                "hard.invalid_results",
                "hard.over_cap_results",
                "hard.transport_errors",
                "hard.maximum_response_bytes",
                "hard.product_disposition_mismatch"
            ]
        );
    }

    #[test]
    fn every_partial_performance_and_size_gate_is_independent() {
        let mut value = observed(120, [30; 4]);
        value.positive_step_recall_milli = 899;
        value.full_or_useful_partial_milli = 949;
        value.actionable_incomplete_gap_milli = 949;
        value.unknown_warm_p95_ms = 501;
        value.transport_p95_ns = [1_500_000_001; 4];
        value.complete_response_p95_bytes = 32_769;
        value.unknown_response_p95_bytes = 16_385;
        value.maximum_response_bytes = 65_537;
        let failures = role_failures(
            "automatic",
            QualificationGateKindV1::AutomaticThreshold,
            &value,
            &frozen().automatic,
            CohortRule::Every,
            WILSON_Z,
        )
        .expect("role")
        .failures;
        assert_eq!(failures.len(), 11);
    }

    #[test]
    fn p95_is_nearest_rank_and_revision_native() {
        let mut values = (1..=20).collect::<Vec<_>>();
        assert_eq!(nearest_rank_p95(&mut values), 19);
        let mut value = observed(120, [30; 4]);
        value.transport_p95_ns = [1, 1, 1_500_000_001, 1];
        let failures = role_failures(
            "automatic",
            QualificationGateKindV1::AutomaticThreshold,
            &value,
            &frozen().automatic,
            CohortRule::Every,
            WILSON_Z,
        )
        .expect("role")
        .failures;
        assert_eq!(
            failures
                .iter()
                .filter(|failure| failure.gate_id.contains(".transport."))
                .map(|failure| failure.gate_id.as_str())
                .collect::<Vec<_>>(),
            ["automatic.transport.2025-06-18.p95_ns"]
        );
    }

    fn source_dependency() -> SourceDependencyEvidenceV1 {
        SourceDependencyEvidenceV1 {
            schema: "codestory.proof-availability-source-dependency/v1".to_owned(),
            qualification_source_commit: "b".repeat(40),
            qualification_source_tree: "c".repeat(40),
            dependency_source: SourceDependencyCoordinateV1 {
                range: OracleSourceRangeV1 {
                    path: "crates/codestory-cli/src/stdio_v3.rs".to_owned(),
                    start_byte: 10,
                    end_byte: 20,
                    file_byte_length: 100,
                    sha256: "a".repeat(64),
                },
                file_sha256: "d".repeat(64),
            },
            test_source: SourceDependencyCoordinateV1 {
                range: OracleSourceRangeV1 {
                    path: "crates/codestory-cli/tests/architecture_contracts.rs".to_owned(),
                    start_byte: 30,
                    end_byte: 40,
                    file_byte_length: 200,
                    sha256: "e".repeat(64),
                },
                file_sha256: "f".repeat(64),
            },
            dependency: SourceDependencyKindV1::TransportCannotRepresentKeepDark,
            passing_test: IntegrationDependencyTestV1 {
                test_id: "transport_cannot_represent_keep_dark".to_owned(),
                kind: IntegrationDependencyTestKindV1::TransportCannotRepresentKeepDark,
                status: IntegrationDependencyTestStatusV1::Passed,
            },
        }
    }

    #[test]
    fn derived_observations_publish_raw_counts_unrounded_wilson_and_tamper_evidence() {
        use super::super::contracts::{
            ActivationDecisionReportV1, DECISION_REPORT_SCHEMA, canonical_observations_sha256,
        };

        let (report, corpus, threshold_value) = accepted_fixture::values();
        let summary = QualificationSummaryV1::from_json(report).expect("summary");
        let thresholds = ThresholdsV1::from_json(threshold_value).expect("thresholds");
        let observations = derive_observations(&summary, &thresholds).expect("observations");
        assert_eq!(observations.full_proofs.numerator, 120);
        assert_eq!(observations.full_proofs.denominator, 120);
        assert_eq!(observations.positive_step_recall.numerator, 312);
        assert_eq!(observations.positive_step_recall.denominator, 312);
        assert_eq!(observations.cohorts.len(), 4);
        assert_eq!(observations.cohorts[0].full_proofs.denominator, 30);
        assert!(observations.full_proof_wilson.lower > 0.96);
        assert!(observations.full_proof_wilson.upper <= 1.0);
        assert_eq!(observations.transport_p95.len(), 4);
        let observations_sha256 = canonical_observations_sha256(
            &summary.provenance.results_sha256,
            &summary.provenance.thresholds_sha256,
            &observations,
        )
        .unwrap();
        let mut report = ActivationDecisionReportV1 {
            schema: DECISION_REPORT_SCHEMA.into(),
            results_sha256: summary.provenance.results_sha256.clone(),
            thresholds_sha256: summary.provenance.thresholds_sha256.clone(),
            observations,
            observations_sha256,
            source_dependency: None,
            source_dependency_sha256: None,
            decision: evaluate_activation_decision(
                &summary,
                &CorpusV1::from_json(corpus).expect("corpus"),
                &thresholds,
                None,
            )
            .expect("decision"),
        };
        report.validate().expect("bound decision report");
        report.observations.full_proofs.numerator -= 1;
        report
            .validate()
            .expect_err("derived observation tampering invalidates its digest");
    }

    #[test]
    fn decision_report_binds_delay_outcome_to_the_validated_source_dependency() {
        use super::super::contracts::{
            ActivationDecisionReportV1, DECISION_REPORT_SCHEMA, canonical_observations_sha256,
            canonical_source_dependency_sha256,
        };

        let (report, corpus, threshold_value) = accepted_fixture::values();
        let summary = QualificationSummaryV1::from_json(report).expect("summary");
        let corpus = CorpusV1::from_json(corpus).expect("corpus");
        let thresholds = ThresholdsV1::from_json(threshold_value).expect("thresholds");
        let observations = derive_observations(&summary, &thresholds).expect("observations");
        let observations_sha256 = canonical_observations_sha256(
            &summary.provenance.results_sha256,
            &summary.provenance.thresholds_sha256,
            &observations,
        )
        .expect("observation digest");
        let dependency = source_dependency();
        let source_dependency_sha256 =
            canonical_source_dependency_sha256(&dependency).expect("dependency digest");
        let mut decision =
            evaluate_activation_decision(&summary, &corpus, &thresholds, Some(&dependency))
                .expect("dependency outcome D");
        let report = ActivationDecisionReportV1 {
            schema: DECISION_REPORT_SCHEMA.into(),
            results_sha256: summary.provenance.results_sha256.clone(),
            thresholds_sha256: summary.provenance.thresholds_sha256.clone(),
            observations: observations.clone(),
            observations_sha256: observations_sha256.clone(),
            source_dependency: Some(dependency.clone()),
            source_dependency_sha256: Some(source_dependency_sha256.clone()),
            decision: decision.clone(),
        };
        report.validate().expect("outcome D report");

        let mut wrong_outcome = report.clone();
        wrong_outcome.decision.outcome = ActivationOutcomeV1::KeepProofDark;
        wrong_outcome
            .validate()
            .expect_err("source dependency evidence selects only outcome D");

        let GateFailureDetailV1::SourceDependency { evidence } =
            &mut decision.failed_gates[0].detail
        else {
            panic!("outcome D must carry dependency evidence")
        };
        evidence.passing_test.test_id = "different-passing-test".to_owned();
        ActivationDecisionReportV1 {
            schema: DECISION_REPORT_SCHEMA.into(),
            results_sha256: summary.provenance.results_sha256.clone(),
            thresholds_sha256: summary.provenance.thresholds_sha256.clone(),
            observations,
            observations_sha256,
            source_dependency: Some(dependency),
            source_dependency_sha256: Some(source_dependency_sha256),
            decision,
        }
        .validate()
        .expect_err("decision evidence must equal the validated dependency input");
    }

    #[test]
    fn decision_order_is_automatic_stable_experimental_dark_and_dependency() {
        let thresholds = frozen();
        let automatic = decision_from_observations(&observed(120, [30; 4]), &thresholds, None)
            .expect("automatic A");
        assert!(matches!(
            automatic.outcome,
            ActivationOutcomeV1::PublicExactVerifier
        ));
        assert_eq!(automatic.automatic_thresholds_met, Some(true));

        let stable = decision_from_observations(&observed(60, [15; 4]), &thresholds, None)
            .expect("stable A");
        assert!(matches!(
            stable.outcome,
            ActivationOutcomeV1::PublicExactVerifier
        ));
        assert_eq!(stable.automatic_thresholds_met, Some(false));

        let experimental =
            decision_from_observations(&observed(25, [12, 5, 4, 4]), &thresholds, None)
                .expect("experimental B");
        assert!(matches!(
            experimental.outcome,
            ActivationOutcomeV1::ExperimentalManualVerifier
        ));

        let dark = decision_from_observations(&observed(23, [6, 6, 6, 5]), &thresholds, None)
            .expect("dark C");
        assert!(matches!(dark.outcome, ActivationOutcomeV1::KeepProofDark));

        let dependency = source_dependency();
        let delayed =
            decision_from_observations(&observed(120, [30; 4]), &thresholds, Some(&dependency))
                .expect("dependency D");
        assert!(matches!(
            delayed.outcome,
            ActivationOutcomeV1::DelayFullV3Cut
        ));
        assert_eq!(delayed.automatic_thresholds_met, None);
        assert_eq!(
            delayed.failed_gates[0].gate_id,
            "integration.source_dependency"
        );
    }

    #[test]
    fn hard_failure_selects_dark_and_metrics_never_select_delay() {
        let thresholds = frozen();
        let mut value = observed(120, [30; 4]);
        value.false_contract_proven = 1;
        let decision =
            decision_from_observations(&value, &thresholds, None).expect("hard-failed decision");
        assert!(matches!(
            decision.outcome,
            ActivationOutcomeV1::KeepProofDark
        ));
        assert!(
            decision
                .failed_gates
                .iter()
                .any(|failure| { failure.gate_id == "hard.false_contract_proven" })
        );
    }

    #[test]
    fn decision_is_deterministic_under_cohort_ordering() {
        let thresholds = frozen();
        let first = observed(60, [11, 16, 16, 17]);
        let mut second = first.clone();
        second.full_proofs_by_cohort = first
            .full_proofs_by_cohort
            .iter()
            .rev()
            .map(|(key, value)| (key.clone(), *value))
            .collect();
        let left = serde_json::to_vec(
            &decision_from_observations(&first, &thresholds, None).expect("left decision"),
        )
        .expect("left JSON");
        let right = serde_json::to_vec(
            &decision_from_observations(&second, &thresholds, None).expect("right decision"),
        )
        .expect("right JSON");
        assert_eq!(left, right);
    }

    #[test]
    fn canonical_threshold_file_and_methodology_are_frozen() {
        let bytes =
            include_bytes!("../../../../../benchmarks/proof-availability/thresholds-v1.json");
        let thresholds = frozen();
        thresholds.validate().expect("frozen threshold values");
        let methodology =
            include_bytes!("../../../../../benchmarks/proof-availability/methodology.md");
        assert_eq!(
            format!("{:x}", Sha256::digest(methodology)),
            thresholds.methodology_sha256
        );
        let original_raw = Sha256::digest(bytes);
        let mut changed = bytes.to_vec();
        changed[0] ^= 1;
        assert_ne!(original_raw.as_slice(), Sha256::digest(changed).as_slice());

        let original_identity =
            canonical_thresholds_sha256(&thresholds).expect("canonical threshold identity");
        assert_eq!(
            original_identity,
            "bc9882f2896c43758b361fb1c5c2a570f37a86548101dc07a55e2d7d76b23f7e"
        );
        let mut value = serde_json::to_value(&thresholds).expect("threshold JSON");
        value["automatic"]["minimum_full_proofs"] = serde_json::json!(95);
        let changed: ThresholdsV1 = serde_json::from_value(value).expect("changed DTO shape");
        assert_ne!(
            original_identity,
            canonical_thresholds_sha256(&changed).expect("changed identity")
        );
    }

    #[test]
    fn public_evaluator_validates_inputs_and_derives_an_automatic_decision() {
        let (report, corpus, thresholds) = accepted_fixture::values();
        let summary = QualificationSummaryV1::from_json(report).expect("accepted summary");
        let corpus = CorpusV1::from_json(corpus).expect("accepted corpus");
        let thresholds = ThresholdsV1::from_json(thresholds).expect("accepted thresholds");
        let decision = evaluate_activation_decision(&summary, &corpus, &thresholds, None)
            .expect("evaluated decision");
        assert!(matches!(
            decision.outcome,
            ActivationOutcomeV1::PublicExactVerifier
        ));
        assert_eq!(decision.automatic_thresholds_met, Some(true));
        assert!(decision.failed_gates.is_empty());
    }

    #[test]
    fn public_evaluator_counts_false_proof_and_disposition_mismatch_hard_failures() {
        let (mut report, corpus, thresholds) = accepted_fixture::values();
        report["cases"][0]["negative_mutations"][0]["contract_proven"] = serde_json::json!(true);
        refresh_results_digest(&mut report);
        let summary = QualificationSummaryV1::from_json(report).expect("false-proof summary");
        let corpus = CorpusV1::from_json(corpus).expect("accepted corpus");
        let thresholds = ThresholdsV1::from_json(thresholds).expect("accepted thresholds");
        let decision = evaluate_activation_decision(&summary, &corpus, &thresholds, None)
            .expect("false-proof decision");
        assert!(matches!(
            decision.outcome,
            ActivationOutcomeV1::KeepProofDark
        ));
        assert!(
            decision
                .failed_gates
                .iter()
                .any(|failure| { failure.gate_id == "hard.false_contract_proven" })
        );

        let (mut report, corpus, thresholds) = accepted_fixture::values();
        report["cases"][0]["receipt_evidence"]["observed_receipts"][0]["oracle_comparison"]["kind"] =
            serde_json::json!("mismatched");
        report["cases"][0]["receipt_evidence"]["observed_receipts"][0]["oracle_comparison"]["oracle_step"]
            ["target"]["symbol"] = serde_json::json!("fixture::wrong-target");
        report["cases"][0]["receipt_evidence"]["observed_receipts"][0]["oracle_comparison"]["mismatches"] =
            serde_json::json!(["target"]);
        let step_index =
            report["cases"][0]["receipt_evidence"]["observed_receipts"][0]["step_index"].clone();
        let oracle_step = report["cases"][0]["receipt_evidence"]["observed_receipts"][0]
            ["oracle_comparison"]["oracle_step"]
            .clone();
        report["cases"][0]["receipt_evidence"]["missing_oracle_steps"]
            .as_array_mut()
            .expect("missing step rows")
            .push(serde_json::json!({ "step_index": step_index, "oracle_step": oracle_step }));
        refresh_results_digest(&mut report);
        let summary = QualificationSummaryV1::from_json(report).expect("mismatched summary");
        let corpus = CorpusV1::from_json(corpus).expect("accepted corpus");
        let thresholds = ThresholdsV1::from_json(thresholds).expect("accepted thresholds");
        let decision = evaluate_activation_decision(&summary, &corpus, &thresholds, None)
            .expect("mismatch decision");
        assert!(
            decision
                .failed_gates
                .iter()
                .any(|failure| { failure.gate_id == "hard.product_disposition_mismatch" })
        );
    }

    #[test]
    fn public_evaluator_rejects_summary_and_corpus_threshold_identity_mismatch() {
        let (report, corpus, thresholds) = accepted_fixture::values();
        let mut summary = QualificationSummaryV1::from_json(report).expect("accepted summary");
        summary.provenance.thresholds_sha256 =
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into();
        let corpus = CorpusV1::from_json(corpus).expect("accepted corpus");
        let thresholds = ThresholdsV1::from_json(thresholds).expect("accepted thresholds");
        evaluate_activation_decision(&summary, &corpus, &thresholds, None)
            .expect_err("summary threshold identity mismatch");

        let (report, mut corpus, thresholds) = accepted_fixture::values();
        corpus["thresholds_sha256"] =
            serde_json::json!("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");
        let summary = QualificationSummaryV1::from_json(report).expect("accepted summary");
        let corpus = CorpusV1::from_json(corpus).expect("shaped corpus");
        let thresholds = ThresholdsV1::from_json(thresholds).expect("accepted thresholds");
        evaluate_activation_decision(&summary, &corpus, &thresholds, None)
            .expect_err("corpus threshold freeze mismatch");
    }
}
