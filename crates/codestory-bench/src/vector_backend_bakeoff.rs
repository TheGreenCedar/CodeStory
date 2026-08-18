//! The immutable vector-backend bake-off contract (W6.8, prior art #1196/#1202).
//!
//! This module owns two things and deliberately nothing else:
//!
//! 1. the *declared* comparison inputs — candidate set, workload ladder,
//!    platform set, and the gates a candidate must clear — written down before
//!    any measurement runs, and
//! 2. [`evaluate_disposition`], the fail-closed rule that turns measurements
//!    into either "adopt this candidate" or "retain the incumbent and record a
//!    non-claim".
//!
//! The gate is *softened*: a bake-off that produces no qualifying candidate is
//! a normal, releasable outcome. It leaves the shipped dense lane exactly as it
//! is, keeps the existing semantic degradation counters as the field signal,
//! and records an explicit non-claim. What the gate must never do is let a
//! hopeful measurement swap the backend, so every path that is not a complete,
//! representative, all-platform, all-workload pass over every declared
//! threshold resolves to [`Disposition::RetainIncumbent`].
//!
//! Nothing here reads a measurement out of the environment. The runner
//! (`codestory_vector_backend_bakeoff`) produces measurements; this module only
//! judges them, which is what makes the judgement unit-testable against
//! measurements that never occurred on this host.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

/// Schema of the recorded bake-off result document.
pub const BAKEOFF_SCHEMA: &str = "codestory.vector-backend-bakeoff/v1";

/// Workload ladder inherited from #1340: the largest representative workload is
/// 75,000 vectors, with three smaller rungs beneath it.
pub const WORKLOAD_LADDER: [u64; 4] = [1_000, 10_000, 25_000, 75_000];

/// The rung the timeout gate is stated against.
pub const TARGET_WORKLOAD_VECTORS: u64 = 75_000;

/// Immutable-generation behaviour has to hold on every shipped package before a
/// backend swap is defensible; these are the packages `release-claims.json`
/// declares as supported.
pub const REQUIRED_PLATFORMS: [&str; 3] = ["macos-arm64", "windows-x64", "linux-x64"];

/// The four candidates W6.8 names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateId {
    /// What ships today: an exact cosine scan over the published SQLite
    /// `vectors` table with a bounded top-k window.
    IncumbentSqliteRowScan,
    /// Exact cosine over a contiguous resident `f32` matrix loaded once per
    /// generation.
    ExactResidentMatrix,
    /// The `sqlite-vec` extension.
    SqliteVec,
    /// The USearch HNSW index.
    Usearch,
}

impl CandidateId {
    pub const ALL: [Self; 4] = [
        Self::IncumbentSqliteRowScan,
        Self::ExactResidentMatrix,
        Self::SqliteVec,
        Self::Usearch,
    ];

    pub const INCUMBENT: Self = Self::IncumbentSqliteRowScan;

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::IncumbentSqliteRowScan => "incumbent_sqlite_row_scan",
            Self::ExactResidentMatrix => "exact_resident_matrix",
            Self::SqliteVec => "sqlite_vec",
            Self::Usearch => "usearch",
        }
    }

    pub const fn is_incumbent(self) -> bool {
        matches!(self, Self::IncumbentSqliteRowScan)
    }
}

/// Where the vectors under measurement came from.
///
/// A recall number measured over vectors nobody's embedding model produced
/// says nothing about the product's answer quality, so it cannot authorize a
/// swap however good it looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CorpusProvenance {
    /// Embeddings produced by the shipped model over a real repository corpus.
    Representative,
    /// Deterministic pseudo-random unit vectors. Exercises the scan's cost
    /// model; carries no quality signal.
    Synthetic,
}

impl CorpusProvenance {
    pub const fn is_representative(self) -> bool {
        matches!(self, Self::Representative)
    }
}

/// Why a candidate produced no measurement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotMeasuredReason {
    /// The backend's native dependency is not vendored into this workspace and
    /// cannot be fetched under the offline build contract.
    DependencyNotVendored,
    /// The host running the bake-off cannot produce the required evidence.
    HostUnavailable,
    /// The operator restricted the run.
    NotRequested,
}

impl NotMeasuredReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DependencyNotVendored => "dependency_not_vendored",
            Self::HostUnavailable => "host_unavailable",
            Self::NotRequested => "not_requested",
        }
    }
}

/// The thresholds a candidate must clear, declared before the run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BakeoffGates {
    /// Neighbour window the agreement gate is measured over.
    pub top_k: usize,
    /// Floor for [`WorkloadMeasurement::top_k_agreement`].
    ///
    /// This is `1.0` rather than a recall target picked out of the air. The
    /// shipped dense lane abstains relative to *its own best hit*, so a
    /// candidate that misses the true nearest neighbour does not merely lose a
    /// result — it moves the abstention floor for every other result in the
    /// window, silently changing what the lane claims. Existing quality
    /// thresholds are therefore only preserved by exact agreement.
    pub minimum_top_k_agreement: f64,
    /// Share of queries at the target workload allowed to exceed the semantic
    /// stage budget.
    pub maximum_stage_timeout_rate: f64,
    /// The product's own semantic-stage budget, read out of the retrieval
    /// planner rather than restated here.
    pub semantic_stage_budget_ms: u64,
    /// Resident bytes a candidate may hold for one generation at the target
    /// workload.
    pub maximum_resident_bytes: u64,
    /// Every rung that must be measured.
    pub workload_ladder: Vec<u64>,
    /// Every platform that must report immutable-generation behaviour.
    pub required_platforms: BTreeSet<String>,
}

/// Resident cap: 384 MiB. At the 768-dimension shipped embedding and the 75,000
/// vector target this leaves room for one full generation held as `f32`
/// (~220 MiB) plus its scoring scratch, and refuses a backend that would need a
/// second full copy resident to answer a query.
pub const DEFAULT_MAXIMUM_RESIDENT_BYTES: u64 = 384 * 1024 * 1024;

/// Timeout gate from W6.8: under 1% of queries at the target size.
pub const DEFAULT_MAXIMUM_STAGE_TIMEOUT_RATE: f64 = 0.01;

/// Neighbour window the agreement gate is measured over. The retrieval planner
/// caps the semantic stage at 40 candidates for natural-language queries, so
/// agreement is judged over the whole window the product can consume.
pub const DEFAULT_TOP_K: usize = 40;

impl BakeoffGates {
    /// The declared gates for a run whose semantic-stage budget was read from
    /// the retrieval planner.
    pub fn declared(semantic_stage_budget_ms: u64) -> Self {
        Self {
            top_k: DEFAULT_TOP_K,
            minimum_top_k_agreement: 1.0,
            maximum_stage_timeout_rate: DEFAULT_MAXIMUM_STAGE_TIMEOUT_RATE,
            semantic_stage_budget_ms,
            maximum_resident_bytes: DEFAULT_MAXIMUM_RESIDENT_BYTES,
            workload_ladder: WORKLOAD_LADDER.to_vec(),
            required_platforms: REQUIRED_PLATFORMS
                .iter()
                .map(|platform| (*platform).to_string())
                .collect(),
        }
    }
}

/// One rung of one candidate's measurement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkloadMeasurement {
    pub vectors: u64,
    pub queries: u64,
    /// Symmetric agreement between the neighbour set this candidate served and
    /// the exhaustive exact set the shipped lane would have served, averaged
    /// over the queries.
    ///
    /// Symmetric, not recall: a backend is charged for neighbours it invents
    /// as well as for neighbours it misses, because the served set is what the
    /// packet reasons over. `|served ∩ exact| / |served ∪ exact|`.
    pub top_k_agreement: f64,
    /// Share of queries whose scan exceeded the semantic stage budget.
    pub stage_timeout_rate: f64,
    /// Bytes the candidate held resident to answer queries at this rung.
    pub resident_bytes: u64,
    /// How `resident_bytes` was derived, so a zero is auditable rather than
    /// merely small.
    pub resident_bytes_basis: String,
    pub build_millis: f64,
    pub p50_scan_micros: f64,
    pub p95_scan_micros: f64,
    pub max_scan_micros: f64,
}

/// A candidate's complete measurement across the ladder.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CandidateMeasurement {
    pub corpus_provenance: CorpusProvenance,
    /// Platforms this candidate produced immutable-generation evidence on.
    pub platforms: BTreeSet<String>,
    pub workloads: Vec<WorkloadMeasurement>,
}

/// What a candidate produced.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum CandidateOutcome {
    NotMeasured {
        reason: NotMeasuredReason,
        detail: String,
    },
    Measured(CandidateMeasurement),
}

impl CandidateOutcome {
    pub fn measurement(&self) -> Option<&CandidateMeasurement> {
        match self {
            Self::Measured(measurement) => Some(measurement),
            Self::NotMeasured { .. } => None,
        }
    }
}

/// The bake-off's verdict.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Disposition {
    /// A candidate cleared every declared gate on every rung and platform over
    /// a representative corpus.
    Adopt { candidate: CandidateId },
    /// Nothing qualified. The shipped lane stands, the semantic degradation
    /// counters remain the field signal, and the bake-off claims nothing.
    RetainIncumbent {
        non_claim: String,
        /// One line per candidate explaining why it did not qualify, in
        /// candidate order. Never empty when a candidate exists.
        blocking_reasons: Vec<String>,
    },
}

impl Disposition {
    pub fn adopted(&self) -> Option<CandidateId> {
        match self {
            Self::Adopt { candidate } => Some(*candidate),
            Self::RetainIncumbent { .. } => None,
        }
    }
}

/// The non-claim recorded when nothing qualifies.
pub const RETAIN_INCUMBENT_NON_CLAIM: &str = "This bake-off selects no vector index backend. The \
     shipped exact SQLite dense scan is retained unchanged, no recall, latency, or memory \
     improvement is claimed from it, and the semantic stage degradation counters remain the only \
     field signal for reconsidering the question.";

/// Everything one bake-off run declared and observed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BakeoffResult {
    pub schema: String,
    /// Host the run executed on, e.g. `macos-arm64`.
    pub host: String,
    pub host_detail: String,
    pub recorded_at: String,
    /// Embedding width the vectors carried.
    pub embedding_dim: usize,
    pub corpus_provenance: CorpusProvenance,
    pub corpus_detail: String,
    pub gates: BakeoffGates,
    pub outcomes: BTreeMap<String, CandidateOutcome>,
    pub disposition: Disposition,
    /// Honest statement of what this run could not establish.
    pub limitations: Vec<String>,
}

impl BakeoffResult {
    pub fn outcome(&self, candidate: CandidateId) -> Option<&CandidateOutcome> {
        self.outcomes.get(candidate.as_str())
    }
}

/// Decide the bake-off, fail-closed.
///
/// A candidate is adopted only when *all* of the following hold. Any one of
/// them missing retains the incumbent:
///
/// * it is not the incumbent;
/// * it produced a measurement rather than a `not_measured` record;
/// * that measurement came from a representative corpus;
/// * it measured every rung of the declared ladder, and no rung outside it;
/// * every rung met the agreement, resident-bytes, and timeout gates;
/// * it reported immutable-generation evidence on every required platform;
/// * and the incumbent itself was measured over the same corpus, so the
///   comparison has a baseline rather than a candidate standing alone.
///
/// When several candidates qualify the winner is the lowest p95 scan time at
/// the target workload, with candidate order as a deterministic tie-break.
pub fn evaluate_disposition(
    gates: &BakeoffGates,
    outcomes: &BTreeMap<String, CandidateOutcome>,
) -> Disposition {
    let baseline_measured = outcomes
        .get(CandidateId::INCUMBENT.as_str())
        .and_then(CandidateOutcome::measurement)
        .is_some_and(|measurement| measurement.corpus_provenance.is_representative());

    let mut blocking_reasons = Vec::new();
    let mut qualified: Vec<(CandidateId, f64)> = Vec::new();

    for candidate in CandidateId::ALL {
        if candidate.is_incumbent() {
            continue;
        }
        let name = candidate.as_str();
        let Some(outcome) = outcomes.get(name) else {
            blocking_reasons.push(format!("{name}: absent from this run"));
            continue;
        };
        match candidate_blocker(gates, outcome, baseline_measured) {
            Some(reason) => blocking_reasons.push(format!("{name}: {reason}")),
            None => {
                let p95 = outcome
                    .measurement()
                    .and_then(|measurement| {
                        measurement
                            .workloads
                            .iter()
                            .find(|workload| workload.vectors == TARGET_WORKLOAD_VECTORS)
                    })
                    .map(|workload| workload.p95_scan_micros)
                    .unwrap_or(f64::INFINITY);
                qualified.push((candidate, p95));
            }
        }
    }

    qualified.sort_by(|left, right| {
        left.1
            .partial_cmp(&right.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.0.cmp(&right.0))
    });

    match qualified.first() {
        Some((candidate, _)) => Disposition::Adopt {
            candidate: *candidate,
        },
        None => Disposition::RetainIncumbent {
            non_claim: RETAIN_INCUMBENT_NON_CLAIM.to_string(),
            blocking_reasons,
        },
    }
}

/// The first reason `outcome` cannot be adopted, or `None` when it qualifies.
fn candidate_blocker(
    gates: &BakeoffGates,
    outcome: &CandidateOutcome,
    baseline_measured: bool,
) -> Option<String> {
    let measurement = match outcome {
        CandidateOutcome::NotMeasured { reason, detail } => {
            return Some(format!("not measured ({}): {detail}", reason.as_str()));
        }
        CandidateOutcome::Measured(measurement) => measurement,
    };
    if !measurement.corpus_provenance.is_representative() {
        return Some(
            "measured over a synthetic corpus, which carries no answer-quality signal".to_string(),
        );
    }
    if !baseline_measured {
        return Some(
            "the incumbent was not measured over a representative corpus in the same run"
                .to_string(),
        );
    }
    let missing_platforms = gates
        .required_platforms
        .iter()
        .filter(|platform| !measurement.platforms.contains(*platform))
        .cloned()
        .collect::<Vec<_>>();
    if !missing_platforms.is_empty() {
        return Some(format!(
            "no immutable-generation evidence on {}",
            missing_platforms.join(", ")
        ));
    }
    let measured_rungs = measurement
        .workloads
        .iter()
        .map(|workload| workload.vectors)
        .collect::<BTreeSet<_>>();
    let declared_rungs = gates
        .workload_ladder
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    if measured_rungs != declared_rungs {
        return Some(format!(
            "measured rungs {measured_rungs:?} do not match the declared ladder {declared_rungs:?}"
        ));
    }
    for workload in &measurement.workloads {
        let vectors = workload.vectors;
        if workload.queries == 0 {
            return Some(format!("{vectors} vectors: no queries were run"));
        }
        if !workload.top_k_agreement.is_finite()
            || workload.top_k_agreement < gates.minimum_top_k_agreement
        {
            return Some(format!(
                "{vectors} vectors: top-{} agreement {:.4} is below the {:.4} floor",
                gates.top_k, workload.top_k_agreement, gates.minimum_top_k_agreement
            ));
        }
        if workload.resident_bytes > gates.maximum_resident_bytes {
            return Some(format!(
                "{vectors} vectors: {} resident bytes exceed the {} cap",
                workload.resident_bytes, gates.maximum_resident_bytes
            ));
        }
        if vectors == TARGET_WORKLOAD_VECTORS
            && (!workload.stage_timeout_rate.is_finite()
                || workload.stage_timeout_rate > gates.maximum_stage_timeout_rate)
        {
            return Some(format!(
                "{vectors} vectors: stage timeout rate {:.4} exceeds the {:.4} ceiling at the \
                 target size",
                workload.stage_timeout_rate, gates.maximum_stage_timeout_rate
            ));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn qualifying_workload(vectors: u64) -> WorkloadMeasurement {
        WorkloadMeasurement {
            vectors,
            queries: 200,
            top_k_agreement: 1.0,
            stage_timeout_rate: 0.0,
            resident_bytes: 16 * 1024 * 1024,
            resident_bytes_basis: "test fixture".to_string(),
            build_millis: 12.0,
            p50_scan_micros: 900.0,
            p95_scan_micros: 1_400.0,
            max_scan_micros: 2_000.0,
        }
    }

    fn all_platforms() -> BTreeSet<String> {
        REQUIRED_PLATFORMS
            .iter()
            .map(|platform| (*platform).to_string())
            .collect()
    }

    fn qualifying_measurement() -> CandidateMeasurement {
        CandidateMeasurement {
            corpus_provenance: CorpusProvenance::Representative,
            platforms: all_platforms(),
            workloads: WORKLOAD_LADDER
                .iter()
                .copied()
                .map(qualifying_workload)
                .collect(),
        }
    }

    /// A run in which `exact_resident_matrix` clears every declared gate.
    /// Every negative test below flips exactly one field of this baseline, so
    /// no negative result can be explained by the baseline being unqualified.
    fn qualifying_run() -> BTreeMap<String, CandidateOutcome> {
        let mut outcomes = BTreeMap::new();
        outcomes.insert(
            CandidateId::INCUMBENT.as_str().to_string(),
            CandidateOutcome::Measured(qualifying_measurement()),
        );
        outcomes.insert(
            CandidateId::ExactResidentMatrix.as_str().to_string(),
            CandidateOutcome::Measured(qualifying_measurement()),
        );
        outcomes.insert(
            CandidateId::SqliteVec.as_str().to_string(),
            CandidateOutcome::NotMeasured {
                reason: NotMeasuredReason::DependencyNotVendored,
                detail: "sqlite-vec is not vendored".to_string(),
            },
        );
        outcomes.insert(
            CandidateId::Usearch.as_str().to_string(),
            CandidateOutcome::NotMeasured {
                reason: NotMeasuredReason::DependencyNotVendored,
                detail: "usearch is not vendored".to_string(),
            },
        );
        outcomes
    }

    fn gates() -> BakeoffGates {
        BakeoffGates::declared(250)
    }

    fn mutate(
        candidate: CandidateId,
        edit: impl FnOnce(&mut CandidateMeasurement),
    ) -> BTreeMap<String, CandidateOutcome> {
        let mut outcomes = qualifying_run();
        let entry = outcomes
            .get_mut(candidate.as_str())
            .expect("candidate is present in the qualifying run");
        let CandidateOutcome::Measured(measurement) = entry else {
            panic!("qualifying run measures {}", candidate.as_str());
        };
        edit(measurement);
        outcomes
    }

    fn blocking_reason(disposition: &Disposition, candidate: CandidateId) -> String {
        let Disposition::RetainIncumbent {
            blocking_reasons, ..
        } = disposition
        else {
            panic!("expected the incumbent to be retained, found {disposition:?}");
        };
        blocking_reasons
            .iter()
            .find(|reason| reason.starts_with(candidate.as_str()))
            .unwrap_or_else(|| {
                panic!(
                    "no blocking reason for {} in {blocking_reasons:?}",
                    candidate.as_str()
                )
            })
            .clone()
    }

    #[test]
    fn a_candidate_clearing_every_declared_gate_is_adopted() {
        // Without this the rest of the suite would be satisfied by an
        // evaluator that always retains the incumbent.
        assert_eq!(
            evaluate_disposition(&gates(), &qualifying_run()).adopted(),
            Some(CandidateId::ExactResidentMatrix)
        );
    }

    #[test]
    fn a_synthetic_corpus_never_authorizes_a_swap() {
        let outcomes = mutate(CandidateId::ExactResidentMatrix, |measurement| {
            measurement.corpus_provenance = CorpusProvenance::Synthetic;
        });
        let disposition = evaluate_disposition(&gates(), &outcomes);
        assert_eq!(disposition.adopted(), None);
        assert!(
            blocking_reason(&disposition, CandidateId::ExactResidentMatrix)
                .contains("synthetic corpus"),
            "{disposition:?}"
        );
    }

    #[test]
    fn a_candidate_measured_without_the_incumbent_baseline_is_not_adopted() {
        let mut outcomes = qualifying_run();
        outcomes.insert(
            CandidateId::INCUMBENT.as_str().to_string(),
            CandidateOutcome::NotMeasured {
                reason: NotMeasuredReason::HostUnavailable,
                detail: "baseline host went away".to_string(),
            },
        );
        let disposition = evaluate_disposition(&gates(), &outcomes);
        assert_eq!(disposition.adopted(), None);
        assert!(
            blocking_reason(&disposition, CandidateId::ExactResidentMatrix)
                .contains("incumbent was not measured"),
            "{disposition:?}"
        );
    }

    #[test]
    fn a_missing_platform_blocks_adoption() {
        let outcomes = mutate(CandidateId::ExactResidentMatrix, |measurement| {
            measurement.platforms.remove("windows-x64");
        });
        let disposition = evaluate_disposition(&gates(), &outcomes);
        assert_eq!(disposition.adopted(), None);
        assert!(
            blocking_reason(&disposition, CandidateId::ExactResidentMatrix).contains("windows-x64"),
            "{disposition:?}"
        );
    }

    #[test]
    fn a_ladder_missing_the_target_rung_blocks_adoption() {
        let outcomes = mutate(CandidateId::ExactResidentMatrix, |measurement| {
            measurement
                .workloads
                .retain(|workload| workload.vectors != TARGET_WORKLOAD_VECTORS);
        });
        let disposition = evaluate_disposition(&gates(), &outcomes);
        assert_eq!(disposition.adopted(), None);
        assert!(
            blocking_reason(&disposition, CandidateId::ExactResidentMatrix).contains("ladder"),
            "{disposition:?}"
        );
    }

    #[test]
    fn one_missed_neighbour_at_any_rung_blocks_adoption() {
        // 39 of 40 neighbours is a 2.5% miss. The shipped lane's abstention
        // floor is relative to its own best hit, so this is a quality change,
        // not a rounding difference.
        let outcomes = mutate(CandidateId::ExactResidentMatrix, |measurement| {
            measurement.workloads[1].top_k_agreement = 39.0 / 40.0;
        });
        let disposition = evaluate_disposition(&gates(), &outcomes);
        assert_eq!(disposition.adopted(), None);
        assert!(
            blocking_reason(&disposition, CandidateId::ExactResidentMatrix).contains("agreement"),
            "{disposition:?}"
        );
    }

    #[test]
    fn exceeding_the_resident_cap_blocks_adoption() {
        let outcomes = mutate(CandidateId::ExactResidentMatrix, |measurement| {
            measurement.workloads[3].resident_bytes = DEFAULT_MAXIMUM_RESIDENT_BYTES + 1;
        });
        let disposition = evaluate_disposition(&gates(), &outcomes);
        assert_eq!(disposition.adopted(), None);
        assert!(
            blocking_reason(&disposition, CandidateId::ExactResidentMatrix)
                .contains("resident bytes"),
            "{disposition:?}"
        );
    }

    #[test]
    fn exceeding_the_timeout_rate_at_the_target_size_blocks_adoption() {
        let outcomes = mutate(CandidateId::ExactResidentMatrix, |measurement| {
            let target = measurement
                .workloads
                .iter_mut()
                .find(|workload| workload.vectors == TARGET_WORKLOAD_VECTORS)
                .expect("target rung");
            target.stage_timeout_rate = DEFAULT_MAXIMUM_STAGE_TIMEOUT_RATE + 0.001;
        });
        let disposition = evaluate_disposition(&gates(), &outcomes);
        assert_eq!(disposition.adopted(), None);
        assert!(
            blocking_reason(&disposition, CandidateId::ExactResidentMatrix)
                .contains("stage timeout rate"),
            "{disposition:?}"
        );
    }

    #[test]
    fn a_rung_with_no_queries_cannot_pass_by_vacuous_agreement() {
        let outcomes = mutate(CandidateId::ExactResidentMatrix, |measurement| {
            measurement.workloads[0].queries = 0;
        });
        let disposition = evaluate_disposition(&gates(), &outcomes);
        assert_eq!(disposition.adopted(), None);
        assert!(
            blocking_reason(&disposition, CandidateId::ExactResidentMatrix).contains("no queries"),
            "{disposition:?}"
        );
    }

    #[test]
    fn a_non_finite_agreement_is_a_failure_not_a_pass() {
        let outcomes = mutate(CandidateId::ExactResidentMatrix, |measurement| {
            measurement.workloads[2].top_k_agreement = f64::NAN;
        });
        assert_eq!(evaluate_disposition(&gates(), &outcomes).adopted(), None);
    }

    #[test]
    fn the_incumbent_is_never_adopted_as_the_bake_off_winner() {
        // The incumbent already ships; "adopt the incumbent" would read as a
        // backend change in the record.
        let mut outcomes = BTreeMap::new();
        outcomes.insert(
            CandidateId::INCUMBENT.as_str().to_string(),
            CandidateOutcome::Measured(qualifying_measurement()),
        );
        let disposition = evaluate_disposition(&gates(), &outcomes);
        assert_eq!(disposition.adopted(), None);
    }

    #[test]
    fn an_empty_run_retains_the_incumbent_with_a_reason_per_candidate() {
        let disposition = evaluate_disposition(&gates(), &BTreeMap::new());
        let Disposition::RetainIncumbent {
            non_claim,
            blocking_reasons,
        } = disposition
        else {
            panic!("an empty run must retain the incumbent");
        };
        assert_eq!(non_claim, RETAIN_INCUMBENT_NON_CLAIM);
        assert_eq!(blocking_reasons.len(), CandidateId::ALL.len() - 1);
    }

    #[test]
    fn the_fastest_qualifier_wins_and_the_order_is_deterministic() {
        let mut outcomes = qualifying_run();
        let mut faster = qualifying_measurement();
        for workload in &mut faster.workloads {
            workload.p95_scan_micros = 100.0;
        }
        outcomes.insert(
            CandidateId::Usearch.as_str().to_string(),
            CandidateOutcome::Measured(faster),
        );
        assert_eq!(
            evaluate_disposition(&gates(), &outcomes).adopted(),
            Some(CandidateId::Usearch)
        );
    }

    #[test]
    fn tightening_the_declared_gates_re_decides_an_already_qualifying_run() {
        // The gates are data, not a constant folded into the evaluator: the
        // same measurements must fail when the declared floor moves.
        let mut strict = gates();
        strict.maximum_resident_bytes = 1;
        assert_eq!(
            evaluate_disposition(&strict, &qualifying_run()).adopted(),
            None
        );
    }
}
