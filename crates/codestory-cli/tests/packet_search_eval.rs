use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const FIXTURE_FILE: &str = "production_packet_search_fixtures.json";
const BASELINE_FILE: &str = "production_packet_search_baseline.json";
const LIVE_EVAL_RUN_ID: &str = "packet-search-eval";
const LIVE_EVAL_PROJECT_ENV: &str = "CODESTORY_PACKET_SEARCH_EVAL_PROJECT";
const LIVE_EVAL_COMMAND_TIMEOUT: Duration = Duration::from_secs(600);
const LIVE_EVAL_PROGRESS_INTERVAL: Duration = Duration::from_secs(30);

#[derive(Debug, Deserialize)]
struct FixtureSet {
    schema_version: u32,
    fixtures: Vec<EvalFixture>,
    #[serde(default)]
    diagnostic_fixtures: Vec<EvalFixture>,
}

#[derive(Debug, Deserialize)]
struct EvalFixture {
    id: String,
    prompt: String,
    query: Option<String>,
    category: String,
    mode: EvalMode,
    expected: ExpectedEvidence,
    provenance: FixtureProvenance,
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum EvalMode {
    Packet,
    Search,
    PacketSearch,
}

#[derive(Debug, Deserialize)]
struct ExpectedEvidence {
    files: Vec<String>,
    symbols: Vec<String>,
    anchors: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct FixtureProvenance {
    issue: String,
    owner: String,
    source: String,
    refs: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct Baseline {
    schema_version: u32,
    fixture_file: String,
    k: usize,
    packet_anchor_budget: usize,
    required_full_modes: RequiredFullModes,
    tolerances: Tolerances,
    overall: MetricSummary,
    categories: BTreeMap<String, MetricSummary>,
    source_fixture_verdicts: VerdictBaseline,
    live_verdict_calibration: LiveVerdictCalibration,
}

#[derive(Debug, Deserialize)]
struct VerdictBaseline {
    sufficiency_counts: BTreeMap<String, usize>,
    proof_status_counts: BTreeMap<String, usize>,
    verdict_cause_table: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct LiveVerdictCalibration {
    status: String,
    issue: String,
    #[serde(default)]
    expected: Option<VerdictBaseline>,
}

#[derive(Debug, Deserialize)]
struct RequiredFullModes {
    readiness_mode: String,
    retrieval_mode: String,
}

#[derive(Debug, Deserialize)]
struct Tolerances {
    recall_at_k: f64,
    anchor_in_packet: f64,
    anchor_before_budget: f64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct MetricSummary {
    fixture_count: usize,
    full_mode_fixture_count: usize,
    recall_at_k: f64,
    anchor_in_packet: f64,
    anchor_before_budget: f64,
}

#[derive(Debug)]
struct EvalRun {
    fixture_id: String,
    readiness_mode: String,
    retrieval_mode: String,
    ranked_files: Vec<String>,
    ranked_symbols: Vec<String>,
    packet_text: String,
    anchor_offsets: BTreeMap<String, usize>,
    sufficiency_status: String,
    proof_statuses: Vec<String>,
    verdict_causes: Vec<String>,
}

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("packet_search_eval")
}

fn repo_root() -> PathBuf {
    repo_root_from(std::env::var_os(LIVE_EVAL_PROJECT_ENV))
}

fn repo_root_from(explicit: Option<OsString>) -> PathBuf {
    if let Some(project) = explicit.filter(|path| !path.is_empty()) {
        return PathBuf::from(project);
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("repo root")
        .to_path_buf()
}

fn load_fixture_set() -> FixtureSet {
    let path = fixture_dir().join(FIXTURE_FILE);
    let text = fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!("read fixture file {}: {error}", path.display());
    });
    serde_json::from_str(&text).unwrap_or_else(|error| {
        panic!("parse fixture file {}: {error}", path.display());
    })
}

fn load_baseline() -> Baseline {
    let path = fixture_dir().join(BASELINE_FILE);
    let text = fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!("read baseline file {}: {error}", path.display());
    });
    serde_json::from_str(&text).unwrap_or_else(|error| {
        panic!("parse baseline file {}: {error}", path.display());
    })
}

fn score_runs(fixtures: &[EvalFixture], runs: &[EvalRun], baseline: &Baseline) -> EvalReport {
    let by_id = runs
        .iter()
        .map(|run| (run.fixture_id.as_str(), run))
        .collect::<BTreeMap<_, _>>();
    let mut overall = Accumulator::default();
    let mut categories = BTreeMap::<String, Accumulator>::new();
    let mut sufficiency_counts = BTreeMap::new();
    let mut proof_status_counts = BTreeMap::new();
    let mut verdict_cause_table = BTreeMap::<String, BTreeSet<String>>::new();

    for fixture in fixtures {
        let run = by_id
            .get(fixture.id.as_str())
            .unwrap_or_else(|| panic!("missing eval run for fixture {}", fixture.id));
        let row = score_fixture(fixture, run, baseline);
        overall.add(&row);
        categories
            .entry(fixture.category.clone())
            .or_default()
            .add(&row);
        if row.full_mode {
            match run.sufficiency_status.as_str() {
                "sufficient" => assert!(
                    run.verdict_causes.is_empty(),
                    "sufficient fixture {} must not report blocking verdict causes: {:?}",
                    fixture.id,
                    run.verdict_causes
                ),
                "partial" | "blocked" => assert!(
                    !run.verdict_causes.is_empty(),
                    "non-sufficient fixture {} must report at least one typed verdict cause",
                    fixture.id
                ),
                status => panic!(
                    "full-mode fixture {} emitted unsupported sufficiency status {status:?}",
                    fixture.id
                ),
            }
            *sufficiency_counts
                .entry(run.sufficiency_status.clone())
                .or_insert(0) += 1;
            for status in &run.proof_statuses {
                *proof_status_counts.entry(status.clone()).or_insert(0) += 1;
            }
            verdict_cause_table
                .entry(fixture.category.clone())
                .or_default()
                .extend(run.verdict_causes.iter().cloned());
        }
    }

    EvalReport {
        overall: overall.finish(),
        categories: categories
            .into_iter()
            .map(|(category, accumulator)| (category, accumulator.finish()))
            .collect(),
        sufficiency_counts,
        proof_status_counts,
        verdict_cause_table: verdict_cause_table
            .into_iter()
            .map(|(category, causes)| (category, causes.into_iter().collect()))
            .collect(),
    }
}

fn score_fixture(fixture: &EvalFixture, run: &EvalRun, baseline: &Baseline) -> FixtureScore {
    let full_mode = run.readiness_mode == baseline.required_full_modes.readiness_mode
        && run.retrieval_mode == baseline.required_full_modes.retrieval_mode;
    let ranked = run
        .ranked_files
        .iter()
        .take(baseline.k)
        .chain(run.ranked_symbols.iter().take(baseline.k))
        .cloned()
        .collect::<BTreeSet<_>>();
    let expected_targets = fixture
        .expected
        .files
        .iter()
        .chain(fixture.expected.symbols.iter())
        .collect::<Vec<_>>();
    let retrieved_targets = expected_targets
        .iter()
        .filter(|target| ranked.contains(target.as_str()))
        .count();
    let anchor_in_packet = fixture
        .expected
        .anchors
        .iter()
        .filter(|anchor| run.packet_text.contains(anchor.as_str()))
        .count();
    let anchor_before_budget = fixture
        .expected
        .anchors
        .iter()
        .filter(|anchor| {
            run.anchor_offsets
                .get(anchor.as_str())
                .is_some_and(|offset| *offset <= baseline.packet_anchor_budget)
        })
        .count();

    FixtureScore {
        full_mode,
        recall_at_k: full_mode.then_some(ratio(retrieved_targets, expected_targets.len())),
        anchor_in_packet: full_mode
            .then_some(ratio(anchor_in_packet, fixture.expected.anchors.len())),
        anchor_before_budget: full_mode
            .then_some(ratio(anchor_before_budget, fixture.expected.anchors.len())),
    }
}

fn ratio(numerator: usize, denominator: usize) -> f64 {
    if denominator == 0 {
        1.0
    } else {
        numerator as f64 / denominator as f64
    }
}

#[derive(Debug)]
struct FixtureScore {
    full_mode: bool,
    recall_at_k: Option<f64>,
    anchor_in_packet: Option<f64>,
    anchor_before_budget: Option<f64>,
}

#[derive(Debug)]
struct EvalReport {
    overall: MetricSummary,
    categories: BTreeMap<String, MetricSummary>,
    sufficiency_counts: BTreeMap<String, usize>,
    proof_status_counts: BTreeMap<String, usize>,
    verdict_cause_table: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Default)]
struct Accumulator {
    fixture_count: usize,
    full_mode_fixture_count: usize,
    recall_at_k: f64,
    anchor_in_packet: f64,
    anchor_before_budget: f64,
}

impl Accumulator {
    fn add(&mut self, score: &FixtureScore) {
        self.fixture_count += 1;
        if score.full_mode {
            self.full_mode_fixture_count += 1;
            self.recall_at_k += score.recall_at_k.expect("full-mode recall");
            self.anchor_in_packet += score.anchor_in_packet.expect("full-mode anchors");
            self.anchor_before_budget +=
                score.anchor_before_budget.expect("full-mode anchor budget");
        }
    }

    fn finish(self) -> MetricSummary {
        let denominator = self.full_mode_fixture_count;
        MetricSummary {
            fixture_count: self.fixture_count,
            full_mode_fixture_count: self.full_mode_fixture_count,
            recall_at_k: ratio_f64(self.recall_at_k, denominator),
            anchor_in_packet: ratio_f64(self.anchor_in_packet, denominator),
            anchor_before_budget: ratio_f64(self.anchor_before_budget, denominator),
        }
    }
}

fn ratio_f64(numerator: f64, denominator: usize) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        numerator / denominator as f64
    }
}

fn assert_metric(actual: f64, expected: f64, tolerance: f64, label: &str) {
    assert!(
        actual + tolerance >= expected,
        "{label} regressed: actual={actual:.3} expected={expected:.3} tolerance={tolerance:.3}"
    );
}

fn assert_summary(actual: &MetricSummary, expected: &MetricSummary, tolerances: &Tolerances) {
    assert_eq!(actual.fixture_count, expected.fixture_count);
    assert_eq!(
        actual.full_mode_fixture_count, expected.full_mode_fixture_count,
        "fallback or stale sidecar rows must not count as full retrieval"
    );
    assert_metric(
        actual.recall_at_k,
        expected.recall_at_k,
        tolerances.recall_at_k,
        "recall_at_k",
    );
    assert_metric(
        actual.anchor_in_packet,
        expected.anchor_in_packet,
        tolerances.anchor_in_packet,
        "anchor_in_packet",
    );
    assert_metric(
        actual.anchor_before_budget,
        expected.anchor_before_budget,
        tolerances.anchor_before_budget,
        "anchor_before_budget",
    );
}

#[test]
fn packet_search_eval_fixture_schema_is_owner_directed_and_complete() {
    let fixtures = load_fixture_set();
    let baseline = load_baseline();
    assert_eq!(fixtures.schema_version, 1);
    assert_eq!(baseline.schema_version, 2);
    assert_eq!(baseline.fixture_file, FIXTURE_FILE);
    assert!(baseline.k > 0);
    assert!(baseline.packet_anchor_budget > 0);

    let mut ids = BTreeSet::new();
    let mut categories = BTreeSet::new();
    for fixture in &fixtures.fixtures {
        assert!(
            ids.insert(&fixture.id),
            "duplicate fixture id {}",
            fixture.id
        );
        assert!(
            !fixture.prompt.trim().is_empty()
                || fixture
                    .query
                    .as_deref()
                    .is_some_and(|q| !q.trim().is_empty()),
            "{} must define a prompt or query",
            fixture.id
        );
        assert!(!fixture.category.trim().is_empty());
        assert_eq!(fixture.mode, EvalMode::PacketSearch);
        assert!(!fixture.expected.files.is_empty());
        assert!(!fixture.expected.symbols.is_empty());
        assert!(!fixture.expected.anchors.is_empty());
        assert_eq!(fixture.provenance.issue, "#510");
        assert_eq!(fixture.provenance.owner, "CodeStory evaluation quality");
        assert!(
            fixture
                .provenance
                .source
                .contains("production packet/search")
        );
        assert!(fixture.provenance.refs.iter().any(|r| r == "#475"));
        assert!(fixture.provenance.refs.iter().any(|r| r == "#469"));
        categories.insert(fixture.category.as_str());
    }
    assert_eq!(baseline.categories.len(), categories.len());
    for category in categories {
        assert!(
            baseline.categories.contains_key(category),
            "baseline missing category {category}"
        );
        assert!(
            baseline
                .source_fixture_verdicts
                .verdict_cause_table
                .contains_key(category),
            "baseline missing verdict cause category {category}"
        );
    }
    assert_eq!(
        baseline
            .source_fixture_verdicts
            .sufficiency_counts
            .values()
            .sum::<usize>(),
        fixtures.fixtures.len(),
        "every source fixture needs one accepted sufficiency verdict"
    );
    assert!(
        !baseline
            .source_fixture_verdicts
            .proof_status_counts
            .is_empty()
    );
    for causes in baseline
        .source_fixture_verdicts
        .verdict_cause_table
        .values()
    {
        let sorted_unique = causes.iter().cloned().collect::<BTreeSet<_>>();
        assert_eq!(
            causes,
            &sorted_unique.into_iter().collect::<Vec<_>>(),
            "verdict cause rows must remain sorted and deduplicated"
        );
    }
    assert_eq!(baseline.live_verdict_calibration.status, "pending");
    assert_eq!(baseline.live_verdict_calibration.issue, "#1351");
    assert!(baseline.live_verdict_calibration.expected.is_none());
    let diagnostic_ids = fixtures
        .diagnostic_fixtures
        .iter()
        .map(|fixture| fixture.id.as_str())
        .collect::<BTreeSet<_>>();
    assert!(
        diagnostic_ids.contains("readiness-boundary-broad-query-diagnostic"),
        "broad readiness query diagnostic fixture must stay visible"
    );
}

#[test]
fn packet_search_eval_readiness_fixture_uses_exact_symbol_search_anchor() {
    let fixtures = load_fixture_set();
    let fixture = fixtures
        .fixtures
        .iter()
        .find(|fixture| fixture.id == "readiness-boundary")
        .expect("readiness-boundary fixture");

    assert!(
        fixture
            .query
            .as_deref()
            .is_some_and(|query| query == "LiveSidecarSearch::semantic_search"),
        "readiness fixture search query must preserve the exact symbol anchor"
    );
}

#[test]
fn packet_search_eval_preserves_broad_readiness_query_diagnostic() {
    let fixtures = load_fixture_set();
    let fixture = fixtures
        .diagnostic_fixtures
        .iter()
        .find(|fixture| fixture.id == "readiness-boundary-broad-query-diagnostic")
        .expect("readiness broad-query diagnostic fixture");

    assert_eq!(
        fixture.query.as_deref(),
        Some("LiveSidecarSearch semantic_search retrieval_mode full sidecar unavailable")
    );
    assert_eq!(fixture.provenance.issue, "#569");
    assert!(
        fixture
            .expected
            .symbols
            .iter()
            .any(|symbol| symbol == "LiveSidecarSearch::semantic_search"),
        "broad diagnostic must keep the missed symbol visible"
    );
}

#[test]
fn packet_search_eval_baseline_scores_full_mode_category_breakdowns() {
    let fixtures = load_fixture_set();
    let baseline = load_baseline();
    let readiness_verdict = packet_verdict_evidence(&serde_json::json!({
        "sufficiency": { "status": "partial" },
        "plan": { "obligations": { "claim_obligations": [
            {
                "material": true,
                "proof_status": "reported",
                "reason": "required_carrier_missing"
            },
            { "material": false, "proof_status": "reported" }
        ] } }
    }));
    let placement_verdict = packet_verdict_evidence(&serde_json::json!({
        "sufficiency": { "status": "partial" },
        "plan": { "obligations": { "claim_obligations": [
            {
                "material": true,
                "proof_status": "reported",
                "reason": "required_carrier_missing"
            },
            { "material": false, "proof_status": "reported" }
        ] } }
    }));
    let runs = vec![
        EvalRun {
            fixture_id: "readiness-boundary".to_string(),
            readiness_mode: "ready".to_string(),
            retrieval_mode: "full".to_string(),
            ranked_files: vec![
                "crates/codestory-retrieval/src/sidecar_search.rs".to_string(),
                "crates/codestory-retrieval/src/lib.rs".to_string(),
            ],
            ranked_symbols: vec![
                "LiveSidecarSearch::semantic_search".to_string(),
                "LiveSidecarSearch::layout".to_string(),
            ],
            packet_text: "LiveSidecarSearch::semantic_search is defined in sidecar_search"
                .to_string(),
            anchor_offsets: BTreeMap::from([
                ("LiveSidecarSearch::semantic_search".to_string(), 10),
                ("sidecar_search".to_string(), 52),
            ]),
            sufficiency_status: readiness_verdict.0,
            proof_statuses: readiness_verdict.1,
            verdict_causes: readiness_verdict.2,
        },
        EvalRun {
            fixture_id: "packet-anchor-placement".to_string(),
            readiness_mode: "ready".to_string(),
            retrieval_mode: "full".to_string(),
            ranked_files: vec![
                "crates/codestory-cli/src/output.rs".to_string(),
                "crates/codestory-runtime/src/agent/packet_evidence.rs".to_string(),
            ],
            ranked_symbols: vec![
                "append_search_evidence_packet".to_string(),
                "decorate_search_hit_evidence".to_string(),
            ],
            packet_text: "decorate_search_hit_evidence uses evidence_tier_for_hit".to_string(),
            anchor_offsets: BTreeMap::from([
                ("decorate_search_hit_evidence".to_string(), 0),
                ("evidence_tier_for_hit".to_string(), 34),
            ]),
            sufficiency_status: placement_verdict.0,
            proof_statuses: placement_verdict.1,
            verdict_causes: placement_verdict.2,
        },
    ];

    let report = score_runs(&fixtures.fixtures, &runs, &baseline);
    assert_summary(&report.overall, &baseline.overall, &baseline.tolerances);
    for (category, expected) in &baseline.categories {
        let actual = report
            .categories
            .get(category)
            .unwrap_or_else(|| panic!("missing category report {category}"));
        assert_summary(actual, expected, &baseline.tolerances);
    }
    assert_eq!(
        report.sufficiency_counts,
        baseline.source_fixture_verdicts.sufficiency_counts
    );
    assert_eq!(
        report.proof_status_counts,
        baseline.source_fixture_verdicts.proof_status_counts
    );
    assert_eq!(
        report.verdict_cause_table,
        baseline.source_fixture_verdicts.verdict_cause_table
    );
}

#[test]
fn packet_search_eval_extracts_schema_v2_obligation_counts_and_causes() {
    let packet = serde_json::json!({
        "sufficiency": { "status": "partial" },
        "plan": {
            "obligations": {
                "claim_obligations": [
                    { "material": true, "proof_status": "proven" },
                    {
                        "material": true,
                        "proof_status": "reported",
                        "reason": "required_evidence_edge_missing"
                    }
                ],
                "query_obligations": [
                    {
                        "material": true,
                        "completion": { "status": "completed" }
                    },
                    {
                        "material": true,
                        "completion": {
                            "status": "cancelled",
                            "reason": "stage_deadline"
                        }
                    }
                ]
            }
        }
    });

    let (status, proof_statuses, causes) = packet_verdict_evidence(&packet);
    assert_eq!(status, "partial");
    assert_eq!(proof_statuses, ["proven", "reported"]);
    assert_eq!(causes, ["required_evidence_edge_missing", "stage_deadline"]);
}

#[test]
fn packet_search_eval_causes_follow_only_final_sufficiency_blockers() {
    let sufficient = serde_json::json!({
        "sufficiency": {
            "status": "sufficient",
            "gaps": ["obligation guard_dispatch is reported: diagnostic lead"]
        },
        "budget": { "truncated": true },
        "plan": { "obligations": {
            "claim_obligations": [
                { "material": true, "proof_status": "proven" },
                {
                    "material": false,
                    "proof_status": "unsupported",
                    "reason": "selected_claim_profile_carrier_missing"
                }
            ],
            "query_obligations": [
                {
                    "material": false,
                    "completion": { "status": "cancelled", "reason": "diagnostic_deadline" }
                }
            ]
        } }
    });
    assert!(packet_verdict_evidence(&sufficient).2.is_empty());

    let material_claim = serde_json::json!({
        "sufficiency": {
            "status": "partial",
            "gaps": [
                "obligation requested_claim:0 is reported: exact member missing",
                "obligation guard_dispatch is reported: diagnostic lead"
            ]
        },
        "plan": { "obligations": { "claim_obligations": [
            {
                "material": true,
                "proof_status": "reported",
                "reason": "exact_member_missing"
            },
            {
                "material": false,
                "proof_status": "reported",
                "reason": "selected_claim_profile_carrier_missing"
            }
        ] } }
    });
    assert_eq!(
        packet_verdict_evidence(&material_claim).2,
        ["exact_member_missing"]
    );

    let material_query = serde_json::json!({
        "sufficiency": { "status": "partial" },
        "plan": { "obligations": { "query_obligations": [
            {
                "material": true,
                "completion": { "status": "cancelled", "reason": "stage_deadline" }
            },
            {
                "material": false,
                "completion": { "status": "cancelled", "reason": "diagnostic_deadline" }
            }
        ] } }
    });
    assert_eq!(
        packet_verdict_evidence(&material_query).2,
        ["stage_deadline"]
    );

    let budget_and_gap = serde_json::json!({
        "sufficiency": {
            "status": "blocked",
            "coverage_report": { "budget_omitted": ["citations"] },
            "gaps": ["minimum claim-family coverage not met"]
        },
        "plan": { "obligations": {} }
    });
    assert_eq!(
        packet_verdict_evidence(&budget_and_gap).2,
        [
            "budget_omitted:citations",
            "sufficiency_gap:minimum claim-family coverage not met"
        ]
    );
}

#[test]
fn packet_search_eval_does_not_count_non_full_retrieval_as_full() {
    let fixtures = load_fixture_set();
    let baseline = load_baseline();
    let runs = fixtures
        .fixtures
        .iter()
        .map(|fixture| EvalRun {
            fixture_id: fixture.id.clone(),
            readiness_mode: "repair_index".to_string(),
            retrieval_mode: "unavailable".to_string(),
            ranked_files: fixture.expected.files.clone(),
            ranked_symbols: fixture.expected.symbols.clone(),
            packet_text: fixture.expected.anchors.join(" "),
            anchor_offsets: fixture
                .expected
                .anchors
                .iter()
                .enumerate()
                .map(|(index, anchor)| (anchor.clone(), index))
                .collect(),
            sufficiency_status: "partial".to_string(),
            proof_statuses: vec!["reported".to_string()],
            verdict_causes: vec!["non_full_mode".to_string()],
        })
        .collect::<Vec<_>>();

    let report = score_runs(&fixtures.fixtures, &runs, &baseline);
    assert_eq!(report.overall.fixture_count, fixtures.fixtures.len());
    assert_eq!(report.overall.full_mode_fixture_count, 0);
    assert_eq!(report.overall.recall_at_k, 0.0);
    assert_eq!(report.overall.anchor_in_packet, 0.0);
    assert_eq!(report.overall.anchor_before_budget, 0.0);
}

#[test]
fn packet_search_eval_readiness_mode_uses_verdict_status_not_sidecar_mode() {
    let readiness = serde_json::json!({
        "verdicts": [
            {
                "goal": "agent_packet_search",
                "status": "repair_index",
                "sidecar": {
                    "retrieval_mode": "full"
                }
            }
        ]
    });

    assert_eq!(readiness_mode(&readiness), "repair_index");
}

#[test]
fn packet_search_eval_reads_production_search_hit_fields() {
    let search = serde_json::json!({
        "indexed_symbol_hits": [
            {
                "file_path": "crates/codestory-cli/src/main.rs",
                "display_name": "run_packet"
            }
        ],
        "repo_text_hits": [
            {
                "file_path": "docs/testing/search-quality-eval.md",
                "display_name": "Search Quality Eval Harness"
            }
        ]
    });

    assert_eq!(
        ranked_files(&search),
        vec![
            "crates/codestory-cli/src/main.rs".to_string(),
            "docs/testing/search-quality-eval.md".to_string()
        ]
    );
    assert_eq!(
        ranked_symbols(&search),
        vec![
            "run_packet".to_string(),
            "Search Quality Eval Harness".to_string()
        ]
    );
}

#[test]
fn packet_search_live_eval_uses_fixed_run_id() {
    assert_ne!(LIVE_EVAL_RUN_ID, "shared-agent");
    assert_eq!(
        live_eval_env(),
        [
            ("CODESTORY_RETRIEVAL_PROFILE", "agent"),
            ("CODESTORY_RETRIEVAL_RUN_ID", LIVE_EVAL_RUN_ID)
        ]
    );
    assert_eq!(
        live_eval_ready_args(),
        [
            "retrieval",
            "index",
            "--profile",
            "agent",
            "--refresh",
            "auto",
            "--run-id",
            LIVE_EVAL_RUN_ID,
            "--format",
            "json"
        ]
    );
    assert_eq!(
        live_eval_status_args(),
        [
            "retrieval",
            "status",
            "--profile",
            "agent",
            "--run-id",
            LIVE_EVAL_RUN_ID,
            "--format",
            "json"
        ]
    );
    let search = live_eval_command(
        Path::new("C:/repo"),
        &[
            "search",
            "--query",
            "LiveSidecarSearch::semantic_search",
            "--profile",
            "agent",
            "--run-id",
            LIVE_EVAL_RUN_ID,
            "--format",
            "json",
        ],
    );
    assert_eq!(search.get_program(), live_eval_cli_path().as_os_str());
    assert!(
        search
            .get_envs()
            .any(|(name, value)| name == "CODESTORY_RETRIEVAL_RUN_ID"
                && value == Some(std::ffi::OsStr::new(LIVE_EVAL_RUN_ID))),
        "search subprocess must inherit the fixed live eval run id"
    );
    assert!(
        search
            .get_envs()
            .any(|(name, value)| name == "CODESTORY_CACHE_ROOT" && value.is_some()),
        "search subprocess must retain the integration-test state root"
    );
    let search_args = search
        .get_args()
        .map(|arg| arg.to_string_lossy().to_string())
        .collect::<Vec<_>>();
    assert!(
        search_args
            .windows(2)
            .any(|pair| pair[0] == "--run-id" && pair[1] == LIVE_EVAL_RUN_ID),
        "search subprocess must pass the fixed live eval run id as a CLI argument"
    );
    let packet = live_eval_command(
        Path::new("C:/repo"),
        &[
            "packet",
            "--question",
            "How does packet search work?",
            "--profile",
            "agent",
            "--run-id",
            LIVE_EVAL_RUN_ID,
            "--format",
            "json",
        ],
    );
    assert!(
        packet
            .get_envs()
            .any(|(name, value)| name == "CODESTORY_RETRIEVAL_PROFILE"
                && value == Some(std::ffi::OsStr::new("agent"))),
        "packet subprocess must inherit the fixed live eval profile"
    );
    let packet_args = packet
        .get_args()
        .map(|arg| arg.to_string_lossy().to_string())
        .collect::<Vec<_>>();
    assert!(
        packet_args
            .windows(2)
            .any(|pair| pair[0] == "--run-id" && pair[1] == LIVE_EVAL_RUN_ID),
        "packet subprocess must pass the fixed live eval run id as a CLI argument"
    );
}

#[test]
fn packet_search_live_eval_honors_explicit_cli_path() {
    let explicit = PathBuf::from("C:/tools/codestory-cli.exe");
    assert_eq!(
        live_eval_cli_path_from(Some(explicit.clone().into_os_string())),
        explicit
    );
    assert_eq!(
        live_eval_cli_path_from(None),
        test_support::cli_binary_path()
    );
}

#[test]
fn packet_search_live_eval_honors_explicit_project_path() {
    let explicit = PathBuf::from("C:/projects/codestory-eval");
    assert_eq!(
        repo_root_from(Some(explicit.clone().into_os_string())),
        explicit
    );
    assert_eq!(repo_root_from(None), repo_root());
}

#[test]
#[ignore = "live production packet/search eval; requires CODESTORY_CLI to name a production binary with an embedded model"]
fn packet_search_eval_live_runs_production_cli_path() {
    let fixtures = load_fixture_set();
    let baseline = load_baseline();
    let project = repo_root();
    let eval_started = Instant::now();
    let readiness = live_eval_run_cli(&project, &live_eval_ready_args());
    assert!(
        readiness.status.success(),
        "agent readiness failed: {}",
        String::from_utf8_lossy(&readiness.stderr)
    );
    let readiness_json: Value =
        serde_json::from_slice(&readiness.stdout).expect("parse readiness json");
    let readiness_mode = readiness_mode(&readiness_json);

    let status = live_eval_run_cli(&project, &live_eval_status_args());
    assert!(
        status.status.success(),
        "retrieval status failed: {}",
        String::from_utf8_lossy(&status.stderr)
    );
    let status_json: Value = serde_json::from_slice(&status.stdout).expect("parse status json");
    let retrieval_mode = status_json["retrieval_mode"]
        .as_str()
        .unwrap_or("unavailable")
        .to_string();
    assert_eq!(readiness_mode, baseline.required_full_modes.readiness_mode);
    assert_eq!(retrieval_mode, baseline.required_full_modes.retrieval_mode);
    assert_live_eval_device_truth(&status_json);

    let mut runs = Vec::new();
    for (index, fixture) in fixtures.fixtures.iter().enumerate() {
        let fixture_started = Instant::now();
        eprintln!(
            "packet_search_eval: fixture {}/{} `{}` started after {:.1}s",
            index + 1,
            fixtures.fixtures.len(),
            fixture.id,
            eval_started.elapsed().as_secs_f64()
        );
        let query = fixture.query.as_deref().unwrap_or(&fixture.prompt);
        let search = live_eval_run_cli(
            &project,
            &[
                "search",
                "--query",
                query,
                "--limit",
                &baseline.k.to_string(),
                "--refresh",
                "none",
                "--why",
                "--profile",
                "agent",
                "--run-id",
                LIVE_EVAL_RUN_ID,
                "--format",
                "json",
            ],
        );
        assert!(
            search.status.success(),
            "search failed for {}: {}",
            fixture.id,
            String::from_utf8_lossy(&search.stderr)
        );
        let search_json: Value = serde_json::from_slice(&search.stdout).expect("parse search json");

        let packet = live_eval_run_cli(
            &project,
            &[
                "packet",
                "--question",
                &fixture.prompt,
                "--budget",
                "compact",
                "--refresh",
                "none",
                "--profile",
                "agent",
                "--run-id",
                LIVE_EVAL_RUN_ID,
                "--format",
                "json",
            ],
        );
        assert!(
            packet.status.success(),
            "packet failed for {}: {}",
            fixture.id,
            String::from_utf8_lossy(&packet.stderr)
        );
        let packet_text = String::from_utf8(packet.stdout).expect("packet stdout utf8");
        let packet_json: Value =
            serde_json::from_str(&packet_text).expect("parse packet verdict json");
        let (sufficiency_status, proof_statuses, verdict_causes) =
            packet_verdict_evidence(&packet_json);
        let anchor_offsets = fixture
            .expected
            .anchors
            .iter()
            .filter_map(|anchor| {
                packet_text
                    .find(anchor)
                    .map(|offset| (anchor.clone(), offset))
            })
            .collect();
        runs.push(EvalRun {
            fixture_id: fixture.id.clone(),
            readiness_mode: readiness_mode.clone(),
            retrieval_mode: retrieval_mode.clone(),
            ranked_files: ranked_files(&search_json),
            ranked_symbols: ranked_symbols(&search_json),
            packet_text,
            anchor_offsets,
            sufficiency_status,
            proof_statuses,
            verdict_causes,
        });
        eprintln!(
            "packet_search_eval: fixture {}/{} `{}` finished in {:.1}s total_elapsed={:.1}s",
            index + 1,
            fixtures.fixtures.len(),
            fixture.id,
            fixture_started.elapsed().as_secs_f64(),
            eval_started.elapsed().as_secs_f64()
        );
    }

    let report = score_runs(&fixtures.fixtures, &runs, &baseline);
    assert_summary(&report.overall, &baseline.overall, &baseline.tolerances);
    for (category, expected) in &baseline.categories {
        let actual = report
            .categories
            .get(category)
            .unwrap_or_else(|| panic!("missing category report {category}"));
        assert_summary(actual, expected, &baseline.tolerances);
    }
    assert_live_verdict_calibration(&report, &baseline, &fixtures.fixtures);
    println!(
        "packet_search_eval recall_at_k={:.3} anchor_in_packet={:.3} anchor_before_budget={:.3} categories={:?} sufficiency_counts={:?} proof_status_counts={:?} verdict_cause_table={:?}",
        report.overall.recall_at_k,
        report.overall.anchor_in_packet,
        report.overall.anchor_before_budget,
        report.categories,
        report.sufficiency_counts,
        report.proof_status_counts,
        report.verdict_cause_table,
    );
}

fn assert_live_verdict_calibration(
    report: &EvalReport,
    baseline: &Baseline,
    fixtures: &[EvalFixture],
) {
    match baseline.live_verdict_calibration.status.as_str() {
        "calibrated" => {
            let expected = baseline
                .live_verdict_calibration
                .expected
                .as_ref()
                .expect("calibrated live verdict baseline must define expected values");
            assert_eq!(report.sufficiency_counts, expected.sufficiency_counts);
            assert_eq!(report.proof_status_counts, expected.proof_status_counts);
            assert_eq!(report.verdict_cause_table, expected.verdict_cause_table);
        }
        "pending" => {
            assert_eq!(baseline.live_verdict_calibration.issue, "#1351");
            assert!(
                baseline.live_verdict_calibration.expected.is_none(),
                "pending live calibration must not masquerade source-fixture values as observed truth"
            );
            assert_eq!(
                report.sufficiency_counts.values().sum::<usize>(),
                fixtures.len(),
                "the live run must emit one sufficiency verdict per full-mode fixture"
            );
            assert!(
                !report.proof_status_counts.is_empty(),
                "the live run must expose obligation proof statuses"
            );
            let expected_categories = fixtures
                .iter()
                .map(|fixture| fixture.category.as_str())
                .collect::<BTreeSet<_>>();
            let actual_categories = report
                .verdict_cause_table
                .keys()
                .map(String::as_str)
                .collect::<BTreeSet<_>>();
            assert_eq!(actual_categories, expected_categories);
        }
        status => panic!("unsupported live verdict calibration status {status:?}"),
    }
}

fn packet_verdict_evidence(packet: &Value) -> (String, Vec<String>, Vec<String>) {
    let packet = packet.get("result").unwrap_or(packet);
    let sufficiency_status = packet
        .pointer("/sufficiency/status")
        .and_then(Value::as_str)
        .unwrap_or("missing")
        .to_string();
    let obligations = packet
        .pointer("/plan/obligations/claim_obligations")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let proof_statuses = obligations
        .iter()
        .filter_map(|obligation| obligation.get("proof_status").and_then(Value::as_str))
        .map(str::to_string)
        .collect::<Vec<_>>();
    if sufficiency_status == "sufficient" {
        return (sufficiency_status, proof_statuses, Vec::new());
    }

    let mut causes = BTreeSet::new();
    for obligation in &obligations {
        if obligation.get("material").and_then(Value::as_bool) != Some(true)
            || obligation.get("proof_status").and_then(Value::as_str) == Some("proven")
        {
            continue;
        }
        causes.insert(
            obligation
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or("claim_reason_missing")
                .to_string(),
        );
    }
    if let Some(query_obligations) = packet
        .pointer("/plan/obligations/query_obligations")
        .and_then(Value::as_array)
    {
        for obligation in query_obligations {
            if obligation.get("material").and_then(Value::as_bool) != Some(true) {
                continue;
            }
            if let Some(reason) = obligation
                .pointer("/completion/reason")
                .and_then(Value::as_str)
            {
                causes.insert(reason.to_string());
            } else if obligation
                .pointer("/completion/status")
                .and_then(Value::as_str)
                != Some("completed")
            {
                causes.insert("completion_missing".to_string());
            }
        }
    }
    if let Some(sections) = packet
        .pointer("/sufficiency/coverage_report/budget_omitted")
        .and_then(Value::as_array)
    {
        for section in sections.iter().filter_map(Value::as_str) {
            causes.insert(format!("budget_omitted:{section}"));
        }
    }
    if let Some(gaps) = packet
        .pointer("/sufficiency/gaps")
        .and_then(Value::as_array)
    {
        for gap in gaps.iter().filter_map(Value::as_str) {
            if gap.starts_with("obligation ") || gap.starts_with("query obligation ") {
                continue;
            }
            causes.insert(format!("sufficiency_gap:{gap}"));
        }
    }
    (
        sufficiency_status,
        proof_statuses,
        causes.into_iter().collect(),
    )
}

fn live_eval_ready_args() -> [&'static str; 10] {
    [
        "retrieval",
        "index",
        "--profile",
        "agent",
        "--refresh",
        "auto",
        "--run-id",
        LIVE_EVAL_RUN_ID,
        "--format",
        "json",
    ]
}

fn live_eval_status_args() -> [&'static str; 8] {
    [
        "retrieval",
        "status",
        "--profile",
        "agent",
        "--run-id",
        LIVE_EVAL_RUN_ID,
        "--format",
        "json",
    ]
}

fn live_eval_env() -> [(&'static str, &'static str); 2] {
    [
        ("CODESTORY_RETRIEVAL_PROFILE", "agent"),
        ("CODESTORY_RETRIEVAL_RUN_ID", LIVE_EVAL_RUN_ID),
    ]
}

fn live_eval_command(project: &Path, args: &[&str]) -> Command {
    let mut command = base_cli_command(project, args);
    command.envs(live_eval_env());
    command
}

fn live_eval_run_cli(project: &Path, args: &[&str]) -> std::process::Output {
    let command_line = live_eval_command_line(project, args);
    let mut command = live_eval_command(project, args);
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    run_command_with_timeout(&mut command, &command_line, LIVE_EVAL_COMMAND_TIMEOUT)
        .unwrap_or_else(|message| panic!("{message}"))
}

fn base_cli_command(project: &Path, args: &[&str]) -> Command {
    let mut command = test_support::command(live_eval_cli_path());
    command.args(args);
    command.arg("--project").arg(project);
    command
}

fn live_eval_command_line(project: &Path, args: &[&str]) -> String {
    let mut parts = vec![live_eval_cli_path().display().to_string()];
    parts.extend(args.iter().map(|arg| (*arg).to_string()));
    parts.push("--project".to_string());
    parts.push(project.display().to_string());
    parts.join(" ")
}

fn live_eval_cli_path() -> PathBuf {
    live_eval_cli_path_from(std::env::var_os("CODESTORY_CLI"))
}

fn live_eval_cli_path_from(explicit: Option<OsString>) -> PathBuf {
    explicit
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(test_support::cli_binary_path)
}

fn run_command_with_timeout(
    command: &mut Command,
    command_line: &str,
    timeout: Duration,
) -> Result<Output, String> {
    eprintln!("packet_search_eval: child started: `{command_line}`");
    let mut child = command.spawn().map_err(|error| {
        format!("spawn codestory-cli live eval command `{command_line}`: {error}")
    })?;
    let stdout = child.stdout.take().map(read_pipe_in_background);
    let stderr = child.stderr.take().map(read_pipe_in_background);
    let started = Instant::now();
    let mut next_progress = LIVE_EVAL_PROGRESS_INTERVAL;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let stdout = join_pipe(stdout);
                let stderr = join_pipe(stderr);
                eprintln!(
                    "packet_search_eval: child finished in {:.1}s status={status}: `{command_line}`",
                    started.elapsed().as_secs_f64()
                );
                return Ok(Output {
                    status,
                    stdout,
                    stderr,
                });
            }
            Ok(None) if started.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                let stdout = String::from_utf8_lossy(&join_pipe(stdout)).to_string();
                let stderr = String::from_utf8_lossy(&join_pipe(stderr)).to_string();
                return Err(format!(
                    "codestory-cli live eval command timed out after {}s: `{}`\nstdout:\n{}\nstderr:\n{}",
                    timeout.as_secs(),
                    command_line,
                    stdout,
                    stderr
                ));
            }
            Ok(None) => {
                let elapsed = started.elapsed();
                if elapsed >= next_progress {
                    eprintln!(
                        "packet_search_eval: child still running after {:.1}s: `{command_line}`",
                        elapsed.as_secs_f64()
                    );
                    next_progress += LIVE_EVAL_PROGRESS_INTERVAL;
                }
                thread::sleep(Duration::from_millis(250));
            }
            Err(error) => {
                return Err(format!(
                    "poll codestory-cli live eval command `{command_line}`: {error}"
                ));
            }
        }
    }
}

fn read_pipe_in_background<R>(mut pipe: R) -> thread::JoinHandle<Vec<u8>>
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = pipe.read_to_end(&mut bytes);
        bytes
    })
}

fn join_pipe(handle: Option<thread::JoinHandle<Vec<u8>>>) -> Vec<u8> {
    handle
        .and_then(|handle| handle.join().ok())
        .unwrap_or_default()
}

fn assert_live_eval_device_truth(status: &Value) {
    assert_json_path_str(status, &["ownership", "profile"], "agent");
    assert_json_path_suffix(
        status,
        &["ownership", "namespace"],
        &format!("-{LIVE_EVAL_RUN_ID}"),
    );
    assert_json_path_contains(status, &["ownership", "state_file"], LIVE_EVAL_RUN_ID);
    assert_json_path_contains(
        status,
        &["ownership", "cleanup_command"],
        "--profile agent --run-id packet-search-eval",
    );
    assert_json_path_str(status, &["embedding_device_policy"], "accelerator_required");
    assert_json_path_str(status, &["embedding_device_state"], "accelerated");
    assert_json_path_str(
        status,
        &["embedding_device_observation_source"],
        "per_user_server",
    );
    assert_json_path_bool(status, &["embedding_cpu_allowed"], false);
    assert_json_path_bool(status, &["embedding_accelerator_requested"], true);
    let detected_provider = json_path(status, &["embedding_detected_provider"])
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .expect("live eval status must report a detected accelerator provider");
    let detected_gpu = json_path(status, &["embedding_detected_gpu"])
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .expect("live eval status must report a detected accelerator device");
    assert_eq!(
        json_path(status, &["embedding_accelerator_request_provider"]).and_then(Value::as_str),
        Some(detected_provider),
        "the requested and observed accelerator providers must agree: {status:#}"
    );
    assert_eq!(
        json_path(status, &["embedding_accelerator_request_device"]).and_then(Value::as_str),
        Some(detected_gpu),
        "the requested and observed accelerator devices must agree: {status:#}"
    );
    assert!(
        !matches!(
            detected_provider.to_ascii_lowercase().as_str(),
            "cpu" | "test-accelerator"
        ),
        "the live production eval cannot use a CPU or test accelerator: {status:#}"
    );
    assert_json_path_non_empty(status, &["embedding_detected_gpu"]);
}

#[test]
fn packet_search_live_eval_accepts_a_real_accelerator_identity() {
    assert_live_eval_device_truth(&serde_json::json!({
        "ownership": {
            "profile": "agent",
            "namespace": "codestory-agent-packet-search-eval",
            "state_file": "/tmp/codestory-agent-packet-search-eval.json",
            "cleanup_command": "codestory-cli retrieval cleanup --profile agent --run-id packet-search-eval"
        },
        "embedding_device_policy": "accelerator_required",
        "embedding_device_state": "accelerated",
        "embedding_device_observation_source": "per_user_server",
        "embedding_cpu_allowed": false,
        "embedding_accelerator_requested": true,
        "embedding_accelerator_request_provider": "Metal",
        "embedding_accelerator_request_device": "Apple GPU",
        "embedding_detected_provider": "Metal",
        "embedding_detected_gpu": "Apple GPU"
    }));
}

#[test]
#[should_panic(expected = "per_user_server")]
fn packet_search_live_eval_rejects_test_support_accelerator_identity() {
    assert_live_eval_device_truth(&serde_json::json!({
        "ownership": {
            "profile": "agent",
            "namespace": "codestory-agent-packet-search-eval",
            "state_file": "/tmp/codestory-agent-packet-search-eval.json",
            "cleanup_command": "codestory-cli retrieval cleanup --profile agent --run-id packet-search-eval"
        },
        "embedding_device_policy": "accelerator_required",
        "embedding_device_state": "accelerated",
        "embedding_device_observation_source": "test_support",
        "embedding_cpu_allowed": false,
        "embedding_accelerator_requested": true,
        "embedding_accelerator_request_provider": "test-accelerator",
        "embedding_accelerator_request_device": "test-accelerator",
        "embedding_detected_provider": "test-accelerator",
        "embedding_detected_gpu": "test-accelerator"
    }));
}

fn assert_json_path_str(status: &Value, path: &[&str], expected: &str) {
    assert_eq!(
        json_path(status, path).and_then(Value::as_str),
        Some(expected),
        "live eval status must report {}={expected}: {status:#}",
        path.join(".")
    );
}

fn assert_json_path_bool(status: &Value, path: &[&str], expected: bool) {
    assert_eq!(
        json_path(status, path).and_then(Value::as_bool),
        Some(expected),
        "live eval status must report {}={expected}: {status:#}",
        path.join(".")
    );
}

fn assert_json_path_non_empty(status: &Value, path: &[&str]) {
    assert!(
        json_path(status, path)
            .and_then(Value::as_str)
            .is_some_and(|value| !value.is_empty()),
        "live eval status must report non-empty {}: {status:#}",
        path.join(".")
    );
}

fn assert_json_path_contains(status: &Value, path: &[&str], expected: &str) {
    assert!(
        json_path(status, path)
            .and_then(Value::as_str)
            .is_some_and(|value| value.contains(expected)),
        "live eval status must report {} containing {expected}: {status:#}",
        path.join(".")
    );
}

fn assert_json_path_suffix(status: &Value, path: &[&str], expected: &str) {
    assert!(
        json_path(status, path)
            .and_then(Value::as_str)
            .is_some_and(|value| value.ends_with(expected)),
        "live eval status must report {} ending with {expected}: {status:#}",
        path.join(".")
    );
}

fn json_path<'a>(value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    path.iter()
        .try_fold(value, |current, field| current.get(*field))
}

fn readiness_mode(json: &Value) -> String {
    json["verdicts"]
        .as_array()
        .and_then(|verdicts| {
            verdicts.iter().find_map(|verdict| {
                (verdict["goal"].as_str() == Some("agent_packet_search"))
                    .then(|| verdict["status"].as_str())
                    .flatten()
            })
        })
        .unwrap_or("unavailable")
        .to_string()
}

fn ranked_files(json: &Value) -> Vec<String> {
    hits(json)
        .filter_map(|hit| hit["file_path"].as_str().map(str::to_string))
        .collect()
}

fn ranked_symbols(json: &Value) -> Vec<String> {
    hits(json)
        .filter_map(|hit| hit["display_name"].as_str().map(str::to_string))
        .collect()
}

fn hits(json: &Value) -> impl Iterator<Item = &Value> {
    json["indexed_symbol_hits"]
        .as_array()
        .into_iter()
        .flatten()
        .chain(json["repo_text_hits"].as_array().into_iter().flatten())
}
mod test_support;
