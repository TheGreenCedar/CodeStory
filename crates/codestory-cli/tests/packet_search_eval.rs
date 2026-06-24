use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const FIXTURE_FILE: &str = "production_packet_search_fixtures.json";
const BASELINE_FILE: &str = "production_packet_search_baseline.json";

#[derive(Debug, Deserialize)]
struct FixtureSet {
    schema_version: u32,
    fixtures: Vec<EvalFixture>,
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
}

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("packet_search_eval")
}

fn repo_root() -> PathBuf {
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
    }

    EvalReport {
        overall: overall.finish(),
        categories: categories
            .into_iter()
            .map(|(category, accumulator)| (category, accumulator.finish()))
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
    assert_eq!(baseline.schema_version, 1);
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
    }
}

#[test]
fn packet_search_eval_baseline_scores_full_mode_category_breakdowns() {
    let fixtures = load_fixture_set();
    let baseline = load_baseline();
    let runs = vec![
        EvalRun {
            fixture_id: "readiness-boundary".to_string(),
            readiness_mode: "full".to_string(),
            retrieval_mode: "full".to_string(),
            ranked_files: vec![
                "crates/codestory-cli/src/main.rs".to_string(),
                "crates/codestory-runtime/src/agent/retrieval_primary.rs".to_string(),
            ],
            ranked_symbols: vec![
                "run_search".to_string(),
                "SidecarPrimarySearchOutcome".to_string(),
            ],
            packet_text:
                "retrieval_mode must be full before promotion; errors include expected mode=full"
                    .to_string(),
            anchor_offsets: BTreeMap::from([
                ("retrieval_mode".to_string(), 40),
                ("expected mode=full".to_string(), 96),
            ]),
        },
        EvalRun {
            fixture_id: "packet-anchor-placement".to_string(),
            readiness_mode: "full".to_string(),
            retrieval_mode: "full".to_string(),
            ranked_files: vec![
                "crates/codestory-cli/src/main.rs".to_string(),
                "crates/codestory-cli/src/args.rs".to_string(),
            ],
            ranked_symbols: vec!["run_packet".to_string(), "SearchOutput".to_string()],
            packet_text: "run_packet emits packet output; search JSON contains indexed_symbol_hits"
                .to_string(),
            anchor_offsets: BTreeMap::from([
                ("run_packet".to_string(), 5),
                ("indexed_symbol_hits".to_string(), 70),
            ]),
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
#[ignore = "live production packet/search eval; requires retrieval_mode=full sidecars for this checkout"]
fn packet_search_eval_live_runs_production_cli_path() {
    let fixtures = load_fixture_set();
    let baseline = load_baseline();
    let project = repo_root();
    let readiness = run_cli(&project, &["ready", "--goal", "agent", "--format", "json"]);
    assert!(
        readiness.status.success(),
        "agent readiness failed: {}",
        String::from_utf8_lossy(&readiness.stderr)
    );
    let readiness_json: Value =
        serde_json::from_slice(&readiness.stdout).expect("parse readiness json");
    let readiness_mode = readiness_mode(&readiness_json);

    let status = run_cli(&project, &["retrieval", "status", "--format", "json"]);
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

    let mut runs = Vec::new();
    for fixture in &fixtures.fixtures {
        let query = fixture.query.as_deref().unwrap_or(&fixture.prompt);
        let search = run_cli(
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

        let packet = run_cli(
            &project,
            &[
                "packet",
                "--question",
                &fixture.prompt,
                "--budget",
                "compact",
                "--refresh",
                "none",
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
        });
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
    println!(
        "packet_search_eval recall_at_k={:.3} anchor_in_packet={:.3} anchor_before_budget={:.3} categories={:?}",
        report.overall.recall_at_k,
        report.overall.anchor_in_packet,
        report.overall.anchor_before_budget,
        report.categories
    );
}

fn run_cli(project: &Path, args: &[&str]) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_codestory-cli"));
    command.args(args);
    command.arg("--project").arg(project);
    command.output().expect("run codestory-cli")
}

fn readiness_mode(json: &Value) -> String {
    json["verdicts"]
        .as_array()
        .and_then(|verdicts| {
            verdicts.iter().find_map(|verdict| {
                (verdict["goal"].as_str() == Some("agent_packet_search"))
                    .then(|| verdict["sidecar"]["retrieval_mode"].as_str())
                    .flatten()
            })
        })
        .unwrap_or("unavailable")
        .to_string()
}

fn ranked_files(json: &Value) -> Vec<String> {
    hits(json)
        .filter_map(|hit| hit["path"].as_str().map(str::to_string))
        .collect()
}

fn ranked_symbols(json: &Value) -> Vec<String> {
    hits(json)
        .filter_map(|hit| hit["name"].as_str().map(str::to_string))
        .collect()
}

fn hits(json: &Value) -> impl Iterator<Item = &Value> {
    json["indexed_symbol_hits"]
        .as_array()
        .into_iter()
        .flatten()
        .chain(json["repo_text_hits"].as_array().into_iter().flatten())
}
