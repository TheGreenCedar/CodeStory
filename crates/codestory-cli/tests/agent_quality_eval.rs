use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::tempdir;

#[derive(Debug, Deserialize, Serialize)]
struct AgentQualityManifest {
    name: String,
    question: String,
    #[serde(default)]
    local_only: bool,
    #[serde(default)]
    project_root: Option<String>,
    expected_anchors: Vec<String>,
    decisive_evidence: Vec<String>,
    required_bridges: Vec<RequiredBridge>,
    #[serde(default)]
    repo_text_dependencies: Vec<String>,
    claim_ledger: ClaimLedger,
}

#[derive(Debug, Deserialize, Serialize)]
struct RequiredBridge {
    from: String,
    to: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct ClaimLedger {
    claims: Vec<Claim>,
}

#[derive(Debug, Deserialize, Serialize)]
struct Claim {
    id: String,
    text: String,
    confidence: f64,
    state: ClaimState,
    #[serde(default)]
    anchors: Vec<String>,
    #[serde(default)]
    evidence: Vec<Evidence>,
    #[serde(default)]
    material_correction: bool,
    #[serde(default)]
    overclaim: bool,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum ClaimState {
    Anchored,
    Supported,
    Partial,
    Inferred,
    NeedsSourceRead,
    Unsupported,
    ContradictedBySource,
}

#[derive(Debug, Deserialize, Serialize)]
struct Evidence {
    kind: EvidenceKind,
    source_path: String,
    #[serde(default)]
    anchor: Option<String>,
    #[serde(default)]
    bridge: Option<String>,
    #[serde(default)]
    decisive: bool,
    #[serde(default)]
    repo_text_only: bool,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum EvidenceKind {
    Symbol,
    Trail,
    Snippet,
    RepoText,
    SourceTruth,
}

#[derive(Debug, Serialize)]
struct AgentQualityScore {
    manifest: String,
    metrics: AgentQualityMetrics,
    failing_claims: Vec<String>,
    scored_claims: Vec<ScoredClaim>,
}

#[derive(Debug, Serialize)]
struct AgentQualityMetrics {
    anchor_recall: f64,
    decisive_evidence_recall: f64,
    bridge_completeness: f64,
    repo_text_dependency: f64,
    unsupported_claims: usize,
    unsupported_high_confidence: usize,
    overclaim_count: usize,
    material_corrections: usize,
    material_correction_high_confidence: usize,
    confidence_calibration: f64,
}

#[derive(Debug, Serialize)]
struct ScoredClaim {
    id: String,
    state: ClaimState,
    confidence: f64,
    unsupported_high_confidence: bool,
    material_correction_high_confidence: bool,
    overclaim: bool,
}

const MIN_CONFIDENCE_CALIBRATION: f64 = 0.70;
const ALLOW_ZERO_REAL_REPO_EVAL_ENV: &str = "CODESTORY_ALLOW_SKIP_LOCAL_REAL_AGENT_QUALITY";

fn allow_zero_real_repo_eval_value(value: Option<&str>) -> bool {
    matches!(value.map(str::trim), Some("1"))
}

fn allow_zero_real_repo_eval_from_env() -> bool {
    allow_zero_real_repo_eval_value(std::env::var(ALLOW_ZERO_REAL_REPO_EVAL_ENV).ok().as_deref())
}

fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("agent_quality")
}

fn load_manifest(name: &str) -> AgentQualityManifest {
    let path = fixture_root().join(format!("{name}.json"));
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read manifest {}: {error}", path.display()));
    serde_json::from_str(&text)
        .unwrap_or_else(|error| panic!("parse manifest {}: {error}", path.display()))
}

fn score_manifest(manifest: &AgentQualityManifest) -> AgentQualityScore {
    let evidenced_anchors = manifest
        .claim_ledger
        .claims
        .iter()
        .flat_map(|claim| claim.evidence.iter())
        .filter_map(|evidence| evidence.anchor.as_ref())
        .cloned()
        .collect::<BTreeSet<_>>();
    let decisive_anchors = manifest
        .claim_ledger
        .claims
        .iter()
        .flat_map(|claim| claim.evidence.iter())
        .filter(|evidence| evidence.decisive)
        .filter_map(|evidence| evidence.anchor.as_ref())
        .cloned()
        .collect::<BTreeSet<_>>();
    let evidenced_bridges = manifest
        .claim_ledger
        .claims
        .iter()
        .flat_map(|claim| claim.evidence.iter())
        .filter_map(|evidence| evidence.bridge.as_ref())
        .cloned()
        .collect::<BTreeSet<_>>();
    let repo_text_sources = manifest
        .claim_ledger
        .claims
        .iter()
        .flat_map(|claim| claim.evidence.iter())
        .filter(|evidence| evidence.repo_text_only)
        .map(|evidence| evidence.source_path.clone())
        .collect::<BTreeSet<_>>();

    let anchor_recall = recall(
        manifest.expected_anchors.len(),
        manifest
            .expected_anchors
            .iter()
            .filter(|anchor| evidenced_anchors.contains(*anchor))
            .count(),
    );
    let decisive_evidence_recall = recall(
        manifest.decisive_evidence.len(),
        manifest
            .decisive_evidence
            .iter()
            .filter(|anchor| decisive_anchors.contains(*anchor))
            .count(),
    );
    let bridge_completeness = recall(
        manifest.required_bridges.len(),
        manifest
            .required_bridges
            .iter()
            .filter(|bridge| evidenced_bridges.contains(&bridge_key(&bridge.from, &bridge.to)))
            .count(),
    );
    let repo_text_dependency = recall(
        manifest.repo_text_dependencies.len(),
        manifest
            .repo_text_dependencies
            .iter()
            .filter(|source| repo_text_sources.contains(*source))
            .count(),
    );

    let mut unsupported_claims = 0usize;
    let mut unsupported_high_confidence = 0usize;
    let mut overclaim_count = 0usize;
    let mut material_corrections = 0usize;
    let mut material_correction_high_confidence = 0usize;
    let mut confidence_quality = 0.0f64;
    let mut scored_claims = Vec::new();
    let mut failing_claims = Vec::new();

    for claim in &manifest.claim_ledger.claims {
        let unsupported = matches!(
            claim.state,
            ClaimState::Unsupported | ClaimState::ContradictedBySource
        ) || claim.evidence.is_empty();
        let high_confidence_unsupported = unsupported && claim.confidence >= 0.70;
        let high_confidence_material_correction =
            claim.material_correction && claim.confidence >= 0.70;

        unsupported_claims += usize::from(unsupported);
        unsupported_high_confidence += usize::from(high_confidence_unsupported);
        overclaim_count += usize::from(claim.overclaim);
        material_corrections += usize::from(claim.material_correction);
        material_correction_high_confidence += usize::from(high_confidence_material_correction);
        confidence_quality += calibrated_confidence(claim.state, claim.confidence);

        if high_confidence_unsupported || high_confidence_material_correction || claim.overclaim {
            failing_claims.push(claim.id.clone());
        }

        scored_claims.push(ScoredClaim {
            id: claim.id.clone(),
            state: claim.state,
            confidence: claim.confidence,
            unsupported_high_confidence: high_confidence_unsupported,
            material_correction_high_confidence: high_confidence_material_correction,
            overclaim: claim.overclaim,
        });
    }

    AgentQualityScore {
        manifest: manifest.name.clone(),
        metrics: AgentQualityMetrics {
            anchor_recall,
            decisive_evidence_recall,
            bridge_completeness,
            repo_text_dependency,
            unsupported_claims,
            unsupported_high_confidence,
            overclaim_count,
            material_corrections,
            material_correction_high_confidence,
            confidence_calibration: recall_f64(
                manifest.claim_ledger.claims.len(),
                confidence_quality,
            ),
        },
        failing_claims,
        scored_claims,
    }
}

fn validate_local_evidence_paths(manifest: &AgentQualityManifest, project_root: &Path) {
    let mut missing = BTreeSet::new();
    for claim in &manifest.claim_ledger.claims {
        for evidence in &claim.evidence {
            let source_path = Path::new(&evidence.source_path);
            let candidate = if source_path.is_absolute() {
                source_path.to_path_buf()
            } else {
                project_root.join(source_path)
            };
            if !candidate.exists() {
                missing.insert(format!("{} -> {}", claim.id, candidate.display()));
            }
        }
    }

    assert!(
        missing.is_empty(),
        "{} has stale evidence source paths:\n{}",
        manifest.name,
        missing.into_iter().collect::<Vec<_>>().join("\n")
    );
}

fn recall(total: usize, hits: usize) -> f64 {
    if total == 0 {
        1.0
    } else {
        hits as f64 / total as f64
    }
}

fn recall_f64(total: usize, sum: f64) -> f64 {
    if total == 0 { 1.0 } else { sum / total as f64 }
}

fn bridge_key(from: &str, to: &str) -> String {
    format!("{from}->{to}")
}

fn calibrated_confidence(state: ClaimState, confidence: f64) -> f64 {
    let ideal = match state {
        ClaimState::Anchored => 0.90,
        ClaimState::Supported => 0.80,
        ClaimState::Partial => 0.60,
        ClaimState::Inferred => 0.50,
        ClaimState::NeedsSourceRead => 0.35,
        ClaimState::Unsupported => 0.10,
        ClaimState::ContradictedBySource => 0.05,
    };
    (1.0 - (confidence - ideal).abs()).clamp(0.0, 1.0)
}

fn assert_quality_gate(score: &AgentQualityScore) {
    assert_eq!(
        score.metrics.material_correction_high_confidence, 0,
        "{} has high-confidence material corrections: {:?}",
        score.manifest, score.failing_claims
    );
    assert_eq!(
        score.metrics.unsupported_high_confidence, 0,
        "{} has unsupported high-confidence claims: {:?}",
        score.manifest, score.failing_claims
    );
    assert_eq!(
        score.metrics.overclaim_count, 0,
        "{} has overclaims: {:?}",
        score.manifest, score.failing_claims
    );
    assert!(
        score.metrics.confidence_calibration >= MIN_CONFIDENCE_CALIBRATION,
        "{} confidence calibration {:.3} is below {:.3}",
        score.manifest,
        score.metrics.confidence_calibration,
        MIN_CONFIDENCE_CALIBRATION
    );
}

fn write_agent_quality_outputs(manifest: &AgentQualityManifest, output_dir: &Path) {
    let score = score_manifest(manifest);
    fs::create_dir_all(output_dir).expect("create output dir");
    fs::write(
        output_dir.join("agent-quality-report.json"),
        serde_json::to_vec_pretty(&score).expect("serialize score"),
    )
    .expect("write score json");
    fs::write(
        output_dir.join("claim-ledger.scored.json"),
        serde_json::to_vec_pretty(&score.scored_claims).expect("serialize scored claims"),
    )
    .expect("write scored ledger");
    fs::write(
        output_dir.join("agent-quality-report.md"),
        render_markdown_report(manifest, &score),
    )
    .expect("write markdown report");
}

fn render_markdown_report(manifest: &AgentQualityManifest, score: &AgentQualityScore) -> String {
    format!(
        "# Agent Quality Report\n\nManifest: `{}`\n\nQuestion: {}\n\n- anchor_recall: {:.3}\n- decisive_evidence_recall: {:.3}\n- bridge_completeness: {:.3}\n- repo_text_dependency: {:.3}\n- unsupported_claims: {}\n- overclaim_count: {}\n- material_corrections: {}\n- confidence_calibration: {:.3}\n",
        manifest.name,
        manifest.question,
        score.metrics.anchor_recall,
        score.metrics.decisive_evidence_recall,
        score.metrics.bridge_completeness,
        score.metrics.repo_text_dependency,
        score.metrics.unsupported_claims,
        score.metrics.overclaim_count,
        score.metrics.material_corrections,
        score.metrics.confidence_calibration
    )
}

#[test]
fn synthetic_manifest_scores_all_quality_metrics_and_writes_reports() {
    let manifest = load_manifest("synthetic_agent_quality");
    let score = score_manifest(&manifest);

    assert_eq!(score.metrics.anchor_recall, 1.0);
    assert_eq!(score.metrics.decisive_evidence_recall, 1.0);
    assert_eq!(score.metrics.bridge_completeness, 1.0);
    assert_eq!(score.metrics.repo_text_dependency, 1.0);
    assert_eq!(score.metrics.unsupported_claims, 0);
    assert_eq!(score.metrics.overclaim_count, 0);
    assert_eq!(score.metrics.material_corrections, 0);
    assert!(score.metrics.confidence_calibration >= 0.90);
    assert_quality_gate(&score);

    let output_dir = tempdir().expect("output dir");
    write_agent_quality_outputs(&manifest, output_dir.path());
    assert!(output_dir.path().join("agent-quality-report.md").is_file());
    assert!(
        output_dir
            .path()
            .join("agent-quality-report.json")
            .is_file()
    );
    assert!(output_dir.path().join("claim-ledger.scored.json").is_file());
}

#[test]
#[should_panic(expected = "unsupported high-confidence claims")]
fn unsupported_high_confidence_claims_fail_the_quality_gate() {
    let manifest = load_manifest("overclaim_trap");
    let score = score_manifest(&manifest);

    assert!(score.metrics.unsupported_claims >= 1);
    assert!(score.metrics.overclaim_count >= 1);
    assert_quality_gate(&score);
}

#[test]
#[should_panic(expected = "has overclaims")]
fn overclaims_fail_the_quality_gate_even_when_supported() {
    let manifest = AgentQualityManifest {
        name: "supported_overclaim_fixture".to_string(),
        question: "Does the gate reject overclaims?".to_string(),
        local_only: false,
        project_root: None,
        expected_anchors: vec!["real".to_string()],
        decisive_evidence: vec!["real".to_string()],
        required_bridges: Vec::new(),
        repo_text_dependencies: Vec::new(),
        claim_ledger: ClaimLedger {
            claims: vec![Claim {
                id: "overclaim".to_string(),
                text: "Supported evidence exists, but the answer claims more than it proves."
                    .to_string(),
                confidence: 0.8,
                state: ClaimState::Supported,
                anchors: vec!["real".to_string()],
                evidence: vec![Evidence {
                    kind: EvidenceKind::SourceTruth,
                    source_path: "real.rs".to_string(),
                    anchor: Some("real".to_string()),
                    bridge: None,
                    decisive: true,
                    repo_text_only: false,
                }],
                material_correction: false,
                overclaim: true,
            }],
        },
    };
    let score = score_manifest(&manifest);

    assert_eq!(score.metrics.unsupported_high_confidence, 0);
    assert_eq!(score.metrics.material_correction_high_confidence, 0);
    assert_eq!(score.metrics.overclaim_count, 1);
    assert_quality_gate(&score);
}

#[test]
#[should_panic(expected = "confidence calibration")]
fn low_confidence_calibration_fails_the_quality_gate() {
    let manifest = AgentQualityManifest {
        name: "low_calibration_fixture".to_string(),
        question: "Does the gate reject poorly calibrated confidence?".to_string(),
        local_only: false,
        project_root: None,
        expected_anchors: vec!["real".to_string()],
        decisive_evidence: vec!["real".to_string()],
        required_bridges: Vec::new(),
        repo_text_dependencies: Vec::new(),
        claim_ledger: ClaimLedger {
            claims: vec![Claim {
                id: "low-confidence-supported".to_string(),
                text: "A supported claim should not be assigned near-zero confidence.".to_string(),
                confidence: 0.05,
                state: ClaimState::Supported,
                anchors: vec!["real".to_string()],
                evidence: vec![Evidence {
                    kind: EvidenceKind::SourceTruth,
                    source_path: "real.rs".to_string(),
                    anchor: Some("real".to_string()),
                    bridge: None,
                    decisive: true,
                    repo_text_only: false,
                }],
                material_correction: false,
                overclaim: false,
            }],
        },
    };
    let score = score_manifest(&manifest);

    assert_eq!(score.metrics.unsupported_high_confidence, 0);
    assert_eq!(score.metrics.material_correction_high_confidence, 0);
    assert_eq!(score.metrics.overclaim_count, 0);
    assert!(score.metrics.confidence_calibration < MIN_CONFIDENCE_CALIBRATION);
    assert_quality_gate(&score);
}

#[test]
#[should_panic(expected = "high-confidence material corrections")]
fn high_confidence_material_corrections_fail_the_quality_gate() {
    let manifest = load_manifest("material_correction_trap");
    let score = score_manifest(&manifest);

    assert!(score.metrics.material_corrections >= 1);
    assert_quality_gate(&score);
}

#[test]
#[should_panic(expected = "stale evidence source paths")]
fn local_evidence_path_validation_rejects_stale_sources() {
    let output_dir = tempdir().expect("fixture root");
    let root = output_dir.path();
    fs::write(root.join("real.rs"), "fn real() {}\n").expect("write real source");
    let manifest = AgentQualityManifest {
        name: "stale_path_fixture".to_string(),
        question: "Does path validation catch stale evidence?".to_string(),
        local_only: true,
        project_root: None,
        expected_anchors: vec!["real".to_string()],
        decisive_evidence: vec!["real".to_string()],
        required_bridges: Vec::new(),
        repo_text_dependencies: Vec::new(),
        claim_ledger: ClaimLedger {
            claims: vec![
                Claim {
                    id: "valid".to_string(),
                    text: "Valid evidence exists.".to_string(),
                    confidence: 0.8,
                    state: ClaimState::Supported,
                    anchors: vec!["real".to_string()],
                    evidence: vec![Evidence {
                        kind: EvidenceKind::SourceTruth,
                        source_path: "real.rs".to_string(),
                        anchor: Some("real".to_string()),
                        bridge: None,
                        decisive: true,
                        repo_text_only: false,
                    }],
                    material_correction: false,
                    overclaim: false,
                },
                Claim {
                    id: "stale".to_string(),
                    text: "Stale evidence should fail.".to_string(),
                    confidence: 0.8,
                    state: ClaimState::Supported,
                    anchors: vec!["stale".to_string()],
                    evidence: vec![Evidence {
                        kind: EvidenceKind::SourceTruth,
                        source_path: "missing.rs".to_string(),
                        anchor: Some("stale".to_string()),
                        bridge: None,
                        decisive: true,
                        repo_text_only: false,
                    }],
                    material_correction: false,
                    overclaim: false,
                },
            ],
        },
    };

    validate_local_evidence_paths(&manifest, root);
}

#[test]
fn zero_real_repo_eval_escape_hatch_requires_exact_one() {
    assert!(allow_zero_real_repo_eval_value(Some("1")));
    assert!(allow_zero_real_repo_eval_value(Some(" 1 ")));
    for value in [None, Some(""), Some("0"), Some("true"), Some("yes")] {
        assert!(
            !allow_zero_real_repo_eval_value(value),
            "only {ALLOW_ZERO_REAL_REPO_EVAL_ENV}=1 should allow skip-only local evidence; got {value:?}"
        );
    }
}

#[test]
#[ignore = "local-only real-repo evaluator; run on the Windows workstation with sibling repos present"]
fn local_real_repo_manifests_score_or_explicitly_skip_missing_repos() {
    let source_repos = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("cli crate has workspace parent")
        .parent()
        .expect("workspace root exists")
        .parent()
        .expect("codestory checkout has sibling repo parent")
        .to_path_buf();
    let mut evaluated = 0usize;
    let mut skipped = Vec::new();

    for name in [
        "real_sourcetrail",
        "real_codestory",
        "real_rootandruntime",
        "real_batcave",
    ] {
        let manifest = load_manifest(name);
        let project_root = manifest
            .project_root
            .as_ref()
            .map(|relative| source_repos.join(relative))
            .expect("local manifest declares project_root");
        if !project_root.is_dir() {
            skipped.push(format!("{}:{}", manifest.name, project_root.display()));
            eprintln!(
                "skipping local-only agent-quality manifest {}: missing repo {}",
                manifest.name,
                project_root.display()
            );
            continue;
        }

        validate_local_evidence_paths(&manifest, &project_root);
        let score = score_manifest(&manifest);
        assert_quality_gate(&score);
        assert!(
            score.metrics.anchor_recall >= 0.66,
            "{} should preserve most requested anchors",
            manifest.name
        );
        evaluated += 1;
    }

    if evaluated == 0 {
        assert!(
            allow_zero_real_repo_eval_from_env(),
            "local-only real-repo quality evaluator evaluated 0 repos; missing repos: {}. \
Set {ALLOW_ZERO_REAL_REPO_EVAL_ENV}=1 only when intentionally collecting skip-only local evidence.",
            skipped.join(", ")
        );
        eprintln!(
            "intentionally skipping local-only agent-quality manifests because {ALLOW_ZERO_REAL_REPO_EVAL_ENV}=1; missing repos: {}",
            skipped.join(", ")
        );
    }
}
