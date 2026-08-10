use anyhow::{Context, Result, bail};
use codestory_retrieval::semantic_calibration_support::{
    CALIBRATION_CORPUS_SCHEMA_VERSION, CALIBRATION_EDGE_CONTRACT_PATH, CALIBRATION_FEATURE,
    CALIBRATION_FIXTURE_PATH, CALIBRATION_FIXTURE_TRANSFORMATION, CalibrationCandidate,
    CalibrationCaptureIdentity, CalibrationExpectedCall, CalibrationFixtureIdentity,
    CalibrationMetrics, CalibrationPolicy, CalibrationQuery, CalibrationSelection,
    CalibrationSelectionContract, QUERY_VECTOR_CAPTURE_DIR_ENV, SemanticCalibrationCorpus,
    development_queries, hex_bytes, load_attested_corpus, materialize_public_owner_fixture,
    raw_semantic_scan, select_policy, sha256_bytes, sha256_file,
    validate_attested_repository_inputs, validate_vector_artifacts,
};
use codestory_retrieval::{
    SidecarProcessDefaults, SidecarProfile, SidecarRuntimeConfig, SidecarRuntimeDefaults,
    SidecarRuntimeOverrides,
};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

const VECTOR_MANIFEST_FILE: &str = "vector-generation-manifest.json";
const VECTOR_DATABASE_FILE: &str = "vectors.sqlite3";
const CORPUS_FILE: &str = "capture.json";
const CALIBRATION_HOLDOUT_MANIFEST_PATH: &str =
    "benchmarks/tasks/language-expansion-holdout/language-support-ab.task.json";

fn main() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let cli = PathBuf::from(args.next().context("missing path to codestory-cli")?);
    let output_dir = PathBuf::from(args.next().context("missing output directory")?);
    let source_commit = args
        .next()
        .context("missing source commit")?
        .to_string_lossy()
        .into_owned();
    if args.next().is_some() {
        bail!("usage: semantic_abstention_capture <cli> <output-dir> <source-commit>");
    }
    if !cli.is_file() {
        bail!("codestory-cli does not exist at {}", cli.display());
    }
    if output_dir.exists() {
        bail!(
            "refusing to replace existing capture directory {}",
            output_dir.display()
        );
    }
    if source_commit.len() != 40 || !source_commit.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("source commit must be a full 40-character hexadecimal commit id");
    }

    let repository_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .context("resolve repository root")?
        .to_path_buf();
    validate_clean_source_commit(&repository_root, &source_commit)?;
    let fixture_path = repository_root.join(CALIBRATION_FIXTURE_PATH);
    let edge_contract_path = repository_root.join(CALIBRATION_EDGE_CONTRACT_PATH);
    let disjointness_manifest_path = repository_root.join(CALIBRATION_HOLDOUT_MANIFEST_PATH);
    let fixture_source = std::fs::read_to_string(&fixture_path)
        .with_context(|| format!("read {}", fixture_path.display()))?;
    let materialized_source = materialize_public_owner_fixture(&fixture_source)?;
    let temporary = tempfile::tempdir().context("create calibration capture directory")?;
    let project = temporary.path().join("project");
    let cache = temporary.path().join("cache");
    let query_vector_dir = temporary.path().join("query-vectors");
    std::fs::create_dir_all(&project)?;
    std::fs::create_dir_all(&cache)?;
    std::fs::create_dir_all(&query_vector_dir)?;
    std::fs::write(project.join("workflow.rs"), materialized_source.as_bytes())?;

    let output = Command::new(&cli)
        .args(["retrieval", "index", "--project"])
        .arg(&project)
        .arg("--cache-dir")
        .arg(&cache)
        .env("CODESTORY_CACHE_ROOT", &cache)
        .args([
            "--profile",
            "local",
            "--refresh",
            "full",
            "--format",
            "json",
        ])
        .output()
        .with_context(|| format!("run {}", cli.display()))?;
    if !output.status.success() {
        bail!(
            "retrieval index failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let index_result: Value =
        serde_json::from_slice(&output.stdout).context("parse retrieval index capture output")?;
    let manifest = index_result
        .get("manifest")
        .context("retrieval index output omitted its manifest")?;
    let dense_count = manifest
        .get("dense_projection_count")
        .and_then(Value::as_u64)
        .context("retrieval manifest omitted dense_projection_count")?;
    if dense_count != 7 {
        bail!("development fixture must publish exactly seven dense anchors, found {dense_count}");
    }
    let semantic_generation = string_field(manifest, "semantic_generation")?;
    let generation = string_field(manifest, "sidecar_generation")?;
    let input_hash = string_field(manifest, "sidecar_input_hash")?;
    let process_defaults =
        SidecarProcessDefaults::new(cache.clone(), SidecarRuntimeDefaults::default());
    let runtime = SidecarRuntimeConfig::for_project_profile_with_process_defaults(
        Some(&project),
        SidecarProfile::Local,
        None,
        &process_defaults,
        &SidecarRuntimeOverrides::default(),
    );
    let collection_dir = runtime
        .layout
        .semantic_data_dir
        .join("collections")
        .join(semantic_generation);
    let vector_manifest = collection_dir.join(VECTOR_MANIFEST_FILE);
    let vector_database = collection_dir.join(VECTOR_DATABASE_FILE);
    validate_vector_artifacts(&vector_manifest, &vector_database)?;

    let mut queries = Vec::with_capacity(development_queries().len());
    for spec in development_queries() {
        let query_output = Command::new(&cli)
            .args(["retrieval", "query", "--project"])
            .arg(&project)
            .arg("--cache-dir")
            .arg(&cache)
            .env("CODESTORY_CACHE_ROOT", &cache)
            .args(["--format", "json", spec.query])
            .env(QUERY_VECTOR_CAPTURE_DIR_ENV, &query_vector_dir)
            .output()
            .with_context(|| format!("capture query vector for {}", spec.task_id))?;
        if !query_output.status.success() {
            bail!(
                "retrieval query failed for {}: {}",
                spec.task_id,
                String::from_utf8_lossy(&query_output.stderr)
            );
        }
        let query_sha256 = sha256_bytes(spec.query.as_bytes());
        let vector_bytes = std::fs::read(query_vector_dir.join(format!("{query_sha256}.f32le")))
            .with_context(|| {
                format!(
                    "the product query path did not capture a vector for {}; build the CLI with \
                     `cargo build --locked -p codestory-cli --features \
                     codestory-retrieval/semantic-calibration-support`",
                    spec.task_id,
                )
            })?;
        if vector_bytes.len() % 4 != 0 {
            bail!("captured query vector for {} is truncated", spec.task_id);
        }
        let query_vector = vector_bytes
            .chunks_exact(4)
            .map(|chunk| {
                f32::from_bits(u32::from_le_bytes(
                    chunk.try_into().expect("four-byte vector chunk"),
                ))
            })
            .collect::<Vec<_>>();
        let candidates =
            raw_semantic_scan(&vector_database, generation, input_hash, &query_vector, 64)?;
        if candidates.len() != dense_count as usize {
            bail!(
                "raw dense scan for {} returned {} of {dense_count} anchors",
                spec.task_id,
                candidates.len()
            );
        }
        queries.push(CalibrationQuery {
            task_id: spec.task_id.to_string(),
            query: spec.query.to_string(),
            query_sha256,
            query_vector_sha256: sha256_bytes(&vector_bytes),
            query_vector_f32_le_hex: hex_bytes(&vector_bytes),
            expected_call: spec.expected_call.map(
                |(caller, caller_owner, callee_owner, callee)| CalibrationExpectedCall {
                    caller: caller.to_string(),
                    caller_owner: caller_owner.to_string(),
                    callee_owner: callee_owner.to_string(),
                    callee: callee.to_string(),
                },
            ),
            noise_nonce: spec.noise_nonce.map(ToString::to_string),
            candidates: candidates
                .into_iter()
                .map(|hit| CalibrationCandidate {
                    node_id: hit.node_id,
                    document_hash: hit.document_hash,
                    display_name: hit.display_name,
                    file_path: hit.file_path,
                    raw_score_bits: hit.raw_score_bits,
                    rank: hit.rank,
                })
                .collect(),
        });
    }

    let capture_command = concat!(
        "cargo build --locked -p codestory-cli --features ",
        "codestory-retrieval/semantic-calibration-support && ",
        "cargo run --locked -p codestory-retrieval --features ",
        "semantic-calibration-support --example semantic_abstention_capture -- ",
        "target/debug/codestory-cli <output-dir> <source-commit>"
    );
    let mut corpus = SemanticCalibrationCorpus {
        schema_version: CALIBRATION_CORPUS_SCHEMA_VERSION,
        capture: CalibrationCaptureIdentity {
            source_commit,
            capture_feature: CALIBRATION_FEATURE.to_string(),
            cli_sha256: sha256_file(&cli)?,
            vector_generation_manifest_file: VECTOR_MANIFEST_FILE.to_string(),
            vector_generation_manifest_sha256: sha256_file(&vector_manifest)?,
            vector_database_file: VECTOR_DATABASE_FILE.to_string(),
            vector_database_sha256: sha256_file(&vector_database)?,
            capture_command: capture_command.to_string(),
        },
        fixture: CalibrationFixtureIdentity {
            source_path: CALIBRATION_FIXTURE_PATH.to_string(),
            source_sha256: sha256_file(&fixture_path)?,
            edge_contract_path: CALIBRATION_EDGE_CONTRACT_PATH.to_string(),
            edge_contract_sha256: sha256_file(&edge_contract_path)?,
            transformation_id: CALIBRATION_FIXTURE_TRANSFORMATION.to_string(),
            materialized_sha256: sha256_bytes(materialized_source.as_bytes()),
            disjointness_manifest_path: CALIBRATION_HOLDOUT_MANIFEST_PATH.to_string(),
            disjointness_manifest_sha256: sha256_file(&disjointness_manifest_path)?,
        },
        selection_contract: CalibrationSelectionContract::exact_grid(),
        queries,
        selection: CalibrationSelection {
            baseline: CalibrationMetrics::default(),
            policy: CalibrationPolicy {
                absolute_floor_hundredths: 0,
                additive_margin_hundredths: 0,
            },
            metrics: CalibrationMetrics::default(),
        },
    };
    validate_attested_repository_inputs(
        &corpus,
        &repository_root,
        CALIBRATION_HOLDOUT_MANIFEST_PATH,
    )?;
    corpus.selection = select_policy(&corpus)?;

    std::fs::create_dir(&output_dir).with_context(|| format!("create {}", output_dir.display()))?;
    std::fs::copy(&vector_manifest, output_dir.join(VECTOR_MANIFEST_FILE))?;
    std::fs::copy(&vector_database, output_dir.join(VECTOR_DATABASE_FILE))?;
    let mut corpus_bytes = serde_json::to_vec_pretty(&corpus)?;
    corpus_bytes.push(b'\n');
    std::fs::write(output_dir.join(CORPUS_FILE), corpus_bytes)?;
    let replayed = load_attested_corpus(
        &output_dir,
        &repository_root,
        CALIBRATION_HOLDOUT_MANIFEST_PATH,
    )?;
    if replayed != corpus {
        bail!("written semantic calibration corpus changed during replay");
    }
    println!(
        "selected floor={:.2} margin={:.2} baseline={:?} selected={:?}",
        corpus.selection.policy.absolute_floor(),
        corpus.selection.policy.additive_margin(),
        corpus.selection.baseline,
        corpus.selection.metrics
    );
    Ok(())
}

fn string_field<'a>(value: &'a Value, field: &str) -> Result<&'a str> {
    value
        .get(field)
        .and_then(Value::as_str)
        .with_context(|| format!("retrieval manifest omitted {field}"))
}

fn validate_clean_source_commit(repository_root: &Path, expected: &str) -> Result<()> {
    let head = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(repository_root)
        .output()
        .context("resolve semantic calibration source commit")?;
    if !head.status.success() || String::from_utf8_lossy(&head.stdout).trim() != expected {
        bail!("semantic calibration source commit does not match the checked-out HEAD");
    }
    let status = Command::new("git")
        .args(["status", "--porcelain", "--untracked-files=all"])
        .current_dir(repository_root)
        .output()
        .context("inspect semantic calibration source worktree")?;
    if !status.status.success() || !status.stdout.is_empty() {
        bail!("semantic calibration source worktree must be clean");
    }
    Ok(())
}
