//! Post-failure measurement helper, never a product or release qualification route.
//! Uses the unchanged product encoder with an isolated native server namespace.

use anyhow::{Context, Result, bail, ensure};
use clap::Parser;
use codestory_retrieval::{
    EmbeddingEngineIdentity, PerUserEmbeddingClient, ProductEmbeddingClient, SidecarRuntimeConfig,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[path = "../src/bin/codestory_proof_availability/build_provenance.rs"]
mod build_provenance;

const INPUT_CONTRACT: &str = "codestory.embedding-diagnostic-input/v1";
const MAX_INPUT_BYTES: u64 = 32 * 1024 * 1024;
const BATCH_SIZE: usize = 16;

#[derive(Parser)]
struct Args {
    #[arg(long)]
    input: PathBuf,
    #[arg(long)]
    input_sha256: String,
    /// Existing private directory containing cache/ and ipc/; output stays here.
    #[arg(long)]
    state_root: PathBuf,
    #[arg(long)]
    output: PathBuf,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Purpose {
    Query,
    Document,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    id: String,
    purpose: Purpose,
    text: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    contract: String,
    records: Vec<Record>,
}

fn validate_input(input: &Input) -> Result<()> {
    ensure!(input.contract == INPUT_CONTRACT, "input_contract_mismatch");
    ensure!(
        (1..=30_000).contains(&input.records.len()),
        "input_record_count_invalid"
    );
    let mut seen = HashSet::new();
    for record in &input.records {
        ensure!(
            !record.id.trim().is_empty() && record.id.len() <= 512,
            "record_id_invalid"
        );
        ensure!(seen.insert(&record.id), "duplicate_record_id");
        ensure!(
            !record.text.trim().is_empty() && record.text.len() <= 65_536,
            "record_text_invalid"
        );
    }
    Ok(())
}

fn validate_vectors(vectors: &[Vec<f32>], expected: usize) -> Result<()> {
    ensure!(vectors.len() == expected, "vector_count_mismatch");
    for vector in vectors {
        ensure!(
            vector.len() == 768 && vector.iter().all(|x| x.is_finite()),
            "vector_shape_invalid"
        );
        let norm = vector.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>();
        ensure!((norm - 1.0).abs() < 0.001, "vector_not_normalized");
    }
    Ok(())
}

fn private_directory(path: &Path) -> Result<()> {
    ensure!(path.is_absolute(), "state_path_not_absolute");
    let metadata = fs::symlink_metadata(path)?;
    ensure!(
        metadata.is_dir() && !metadata.is_symlink(),
        "state_not_directory"
    );
    ensure!(fs::canonicalize(path)? == path, "state_path_not_canonical");
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        ensure!(
            metadata.uid() == unsafe { libc::geteuid() } && metadata.mode() & 0o077 == 0,
            "state_directory_not_private"
        );
    }
    #[cfg(not(unix))]
    bail!("diagnostic_requires_unix_private_directory_validation");
    Ok(())
}

fn validate_isolation(root: &Path, runtime: &SidecarRuntimeConfig) -> Result<()> {
    private_directory(root)?;
    let cache = root.join("cache");
    let ipc = root.join("ipc");
    private_directory(&cache)?;
    private_directory(&ipc)?;
    ensure!(runtime.cache_root == cache, "diagnostic_cache_not_isolated");
    ensure!(
        !runtime.embedding.allow_cpu,
        "diagnostic_cpu_fallback_forbidden"
    );
    let gate = codestory_retrieval::qualification_gate_environment();
    ensure!(
        gate.directory.as_deref() == Some(ipc.as_os_str()),
        "diagnostic_ipc_not_isolated"
    );
    let nonce = gate.nonce_string().context("diagnostic_nonce_missing")?;
    ensure!(
        (16..=64).contains(&nonce.len())
            && nonce
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
        "diagnostic_nonce_invalid"
    );
    Ok(())
}

fn validate_state(args: &Args, runtime: &SidecarRuntimeConfig) -> Result<()> {
    validate_isolation(&args.state_root, runtime)?;
    ensure!(
        args.output.parent() == Some(args.state_root.as_path()),
        "output_outside_state"
    );
    match fs::symlink_metadata(&args.output) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
        Ok(_) => bail!("output_already_exists"),
    }
}

fn publish_result(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("output_parent_missing")?;
    private_directory(parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    // Atomic no-clobber publication, including a destination racing preflight.
    temporary.persist_noclobber(path)?;
    Ok(())
}

fn read_input(path: &Path, expected: &str) -> Result<Input> {
    let file = fs::File::open(path)?;
    ensure!(file.metadata()?.is_file(), "input_not_regular_file");
    let mut bytes = Vec::new();
    file.take(MAX_INPUT_BYTES + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 <= MAX_INPUT_BYTES, "input_too_large");
    ensure!(
        format!("{:x}", Sha256::digest(&bytes)) == expected,
        "input_digest_mismatch"
    );
    let input = serde_json::from_slice(&bytes)?;
    validate_input(&input)?;
    Ok(input)
}

fn engine_receipt(identity: &EmbeddingEngineIdentity) -> Result<serde_json::Value> {
    ensure!(
        identity.accelerator_execution_verified
            && identity.worker_alive
            && identity.load_error.is_none()
            && identity.embedded_model
            && identity.policy == "accelerated",
        "engine_execution_unverified"
    );
    Ok(serde_json::to_value(identity)?)
}

fn main() -> Result<()> {
    if std::env::args().nth(1).as_deref() == Some("internal-embedding-server") {
        ensure!(
            build_provenance::SOURCE_DIRTY.trim() == "false",
            "dirty_diagnostic_binary"
        );
        let runtime = SidecarRuntimeConfig::local();
        let root = runtime.cache_root.parent().context("state_root_missing")?;
        validate_isolation(root, &runtime)?;
        return codestory_cli::run_native_embedding_server();
    }
    let args = Args::parse();
    let input = read_input(&args.input, &args.input_sha256)?;
    let runtime = SidecarRuntimeConfig::local();
    validate_state(&args, &runtime)?;
    ensure!(
        build_provenance::SOURCE_DIRTY.trim() == "false",
        "dirty_diagnostic_binary"
    );
    let mut executable = fs::File::open(std::env::current_exe()?)?;
    let mut executable_digest = Sha256::new();
    std::io::copy(&mut executable, &mut executable_digest)?;
    codestory_cli::install_native_embedding_client_transport()?;
    let started = Instant::now();
    let mut residency = PerUserEmbeddingClient::for_runtime(&runtime)?.acquire_residency_lease()?;
    let initial_engine = engine_receipt(residency.identity())?;
    let client = ProductEmbeddingClient::new(&runtime);
    let mut results = Vec::with_capacity(input.records.len());
    let mut ordinal = 0;
    while ordinal < input.records.len() {
        let purpose = input.records[ordinal].purpose;
        let end = input.records[ordinal..]
            .iter()
            .take(BATCH_SIZE)
            .take_while(|record| record.purpose == purpose)
            .count()
            + ordinal;
        let texts = input.records[ordinal..end]
            .iter()
            .map(|r| r.text.clone())
            .collect::<Vec<_>>();
        let batch_started = Instant::now();
        let timeout = Some(Duration::from_secs(60));
        let vectors = match purpose {
            Purpose::Query => client.embed_queries_with_control(&texts, timeout, &|| false),
            Purpose::Document => client.embed_documents_with_control(&texts, timeout, &|| false),
        }
        .with_context(|| format!("encode_failed_at_records_{ordinal}_{end}"))?;
        validate_vectors(&vectors, texts.len())?;
        let batch_ms = batch_started.elapsed().as_millis();
        for (record, vector) in input.records[ordinal..end].iter().zip(vectors) {
            results.push(serde_json::json!({
                "id": record.id, "purpose": record.purpose,
                "text_sha256": format!("{:x}", Sha256::digest(record.text.as_bytes())),
                "vector": vector,
            }));
        }
        ordinal = end;
        eprintln!(
            "encoded {ordinal}/{} records; batch_ms={batch_ms}",
            input.records.len()
        );
    }
    let final_engine = engine_receipt(&residency.revalidate()?)?;
    for key in [
        "server_instance_id",
        "load_generation",
        "model_digest",
        "ggml_build_identity",
    ] {
        ensure!(
            initial_engine[key] == final_engine[key],
            "engine_identity_changed: {key}"
        );
    }
    let receipt = serde_json::json!({
        "contract": "codestory.embedding-diagnostic-output/v1",
        "authority": "post_failure_diagnostic_only",
        "packet_decision": "not_evaluated",
        "input_sha256": args.input_sha256,
        "source_commit": build_provenance::SOURCE_COMMIT.trim(),
        "source_tree": build_provenance::SOURCE_TREE.trim(),
        "build_profile": build_provenance::BUILD_PROFILE.trim(),
        "rustc": build_provenance::RUSTC_VV.trim(),
        "binary_sha256": format!("{:x}", executable_digest.finalize()),
        "initial_engine": initial_engine, "final_engine": final_engine,
        "whole_encoding_wall_ms": started.elapsed().as_millis(),
        "records": results,
    });
    // Recheck the owned destination before publication; failed encoding creates no result.
    validate_state(&args, &runtime)?;
    publish_result(&args.output, &serde_json::to_vec(&receipt)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(records: serde_json::Value) -> Input {
        serde_json::from_value(serde_json::json!({
            "contract": INPUT_CONTRACT,
            "records": records,
        }))
        .unwrap()
    }

    #[test]
    fn reject_invalid_input_before_encoder_activation() {
        let valid = serde_json::json!({"id":"q0","purpose":"query","text":"find α"});
        validate_input(&input(serde_json::json!([valid]))).unwrap();
        for records in [
            serde_json::json!([]),
            serde_json::json!([valid, valid]),
            serde_json::json!([{"id":"","purpose":"query","text":"x"}]),
            serde_json::json!([{"id":"q","purpose":"query","text":"  "}]),
            serde_json::json!([{"id":"d","purpose":"document","text":"x".repeat(65_537)}]),
        ] {
            assert!(validate_input(&input(records)).is_err());
        }
        let mut wrong_contract = input(serde_json::json!([valid]));
        wrong_contract.contract = "other".into();
        assert!(validate_input(&wrong_contract).is_err());
    }

    #[test]
    fn reject_missing_extra_nonfinite_or_unnormalized_vectors() {
        let mut unit = vec![0.0; 768];
        unit[0] = 1.0;
        validate_vectors(&[unit.clone()], 1).unwrap();
        assert!(validate_vectors(&[], 1).is_err());
        assert!(validate_vectors(&[unit.clone(), unit.clone()], 1).is_err());
        assert!(validate_vectors(&[vec![1.0]], 1).is_err());
        assert!(validate_vectors(&[vec![0.0; 768]], 1).is_err());
        unit[0] = f32::NAN;
        assert!(validate_vectors(&[unit], 1).is_err());
    }

    #[test]
    fn unknown_fields_and_purposes_cannot_change_the_encoder_contract() {
        for record in [
            serde_json::json!({"id":"d","purpose":"rerank","text":"x"}),
            serde_json::json!({"id":"d","purpose":"document","text":"x","truncate":true}),
        ] {
            assert!(serde_json::from_value::<Record>(record).is_err());
        }
    }

    #[test]
    fn input_digest_binds_the_actual_encoder_text() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("input.json");
        let bytes = serde_json::to_vec(&serde_json::json!({
            "contract": INPUT_CONTRACT,
            "records": [{"id":"d0","purpose":"document","text":"α\nβ\n"}],
        }))
        .unwrap();
        fs::write(&path, &bytes).unwrap();
        let digest = format!("{:x}", Sha256::digest(&bytes));
        assert_eq!(
            read_input(&path, &digest).unwrap().records[0].text,
            "α\nβ\n"
        );
        fs::write(&path, b"{}").unwrap();
        assert!(read_input(&path, &digest).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn state_must_be_private_and_canonical_not_a_symlink() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let directory = tempfile::tempdir().unwrap();
        let canonical = fs::canonicalize(directory.path()).unwrap();
        fs::set_permissions(&canonical, fs::Permissions::from_mode(0o700)).unwrap();
        private_directory(&canonical).unwrap();
        fs::set_permissions(&canonical, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(private_directory(&canonical).is_err());
        fs::set_permissions(&canonical, fs::Permissions::from_mode(0o700)).unwrap();
        let link = canonical.join("alias");
        symlink(&canonical, &link).unwrap();
        assert!(private_directory(&link).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn concurrent_output_is_never_replaced() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let canonical = fs::canonicalize(directory.path()).unwrap();
        fs::set_permissions(&canonical, fs::Permissions::from_mode(0o700)).unwrap();
        let path = canonical.join("result.json");
        publish_result(&path, b"first").unwrap();
        assert!(publish_result(&path, b"second").is_err());
        assert_eq!(fs::read(&path).unwrap(), b"first");
        assert_eq!(fs::read_dir(&canonical).unwrap().count(), 1);
    }
}
