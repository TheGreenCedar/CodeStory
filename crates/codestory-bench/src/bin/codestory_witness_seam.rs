//! Deterministic replay of the frozen sixteen-candidate witness experiment.
//! This binary never retrieves candidates, launches models, or decides a gate.

use anyhow::{Context, Result, ensure};
use clap::{Parser, Subcommand};
use codestory_contracts::compilation::PacketCompilationPublicationV1;
use codestory_runtime::benchmark_support::{
    WitnessSeamDescriptor, freeze_witness_descriptors, run_witness_seam,
};
use codestory_store::CoreReadSession;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

#[path = "codestory_proof_availability/build_provenance.rs"]
mod build_provenance;

#[derive(Parser)]
#[command(about = "Replay frozen descriptors through header and addressed hydration")]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Build a core and isolated lexical shard in a new external directory.
    Prepare {
        #[arg(long)]
        project: PathBuf,
        #[arg(long)]
        output_dir: PathBuf,
    },
    /// Freeze the first sixteen existing lexical hits, without hydration selection.
    Capture {
        #[arg(long)]
        prepared: PathBuf,
        #[arg(long)]
        prepared_sha256: String,
        #[arg(long)]
        case_id: String,
        #[arg(long)]
        phrasing_id: String,
        #[arg(long)]
        question: String,
        #[arg(long)]
        output: PathBuf,
    },
    Replay {
        #[arg(long)]
        manifest: PathBuf,
        #[arg(long)]
        manifest_sha256: String,
        /// A new external receipt file. An existing file is never overwritten.
        #[arg(long)]
        output: PathBuf,
    },
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    contract: String,
    case_id: String,
    phrasing_id: String,
    project_root: PathBuf,
    storage_path: PathBuf,
    publication: PacketCompilationPublicationV1,
    descriptors: Vec<WitnessSeamDescriptor>,
    #[serde(default)]
    capture: Option<serde_json::Value>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Prepared {
    contract: String,
    project_root: PathBuf,
    storage_path: PathBuf,
    lexical_root: PathBuf,
    lexical_input_hash: String,
    publication: PacketCompilationPublicationV1,
    core_pointer: codestory_contracts::core_publication::CorePublicationPointerV1,
}

fn read_manifest(path: &Path, expected: &str) -> Result<Manifest> {
    let bytes = std::fs::read(path).context("read frozen manifest")?;
    ensure!(
        expected.len() == 64 && format!("{:x}", Sha256::digest(&bytes)) == expected,
        "frozen manifest digest mismatch"
    );
    let manifest: Manifest = serde_json::from_slice(&bytes)?;
    ensure!(
        manifest.contract == "codestory.witness-seam-input/v1"
            && !manifest.case_id.is_empty()
            && !manifest.phrasing_id.is_empty(),
        "unsupported or unidentified witness experiment"
    );
    ensure!(
        manifest.project_root.is_absolute() && manifest.storage_path.is_absolute(),
        "experiment paths must be absolute"
    );
    Ok(manifest)
}

fn file_digest(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn write_receipt(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn main() -> Result<()> {
    let args = Args::parse();
    // A dirty binary can exercise tests, but cannot produce experiment evidence.
    ensure!(
        build_provenance::SOURCE_DIRTY.trim() == "false",
        "witness replay requires a clean-source binary"
    );
    match args.command {
        Command::Prepare {
            project,
            output_dir,
        } => prepare(&project, &output_dir),
        Command::Capture {
            prepared,
            prepared_sha256,
            case_id,
            phrasing_id,
            question,
            output,
        } => capture(
            &prepared,
            &prepared_sha256,
            case_id,
            phrasing_id,
            &question,
            &output,
        ),
        Command::Replay {
            manifest,
            manifest_sha256,
            output,
        } => replay(&manifest, &manifest_sha256, &output),
    }
}

fn prepare(project: &Path, output: &Path) -> Result<()> {
    use codestory_contracts::workspace::SourceIndexPolicy;
    use codestory_runtime::{
        RetrievalProcessDefaults, RetrievalRuntimeDefaults, RetrievalRuntimeOverrides, Runtime,
        RuntimeProcessConfig, RuntimeRetrievalConfig, RuntimeRetrievalProfile,
    };
    let project = project.canonicalize()?;
    ensure!(
        output.is_absolute() && !output.starts_with(&project),
        "preparation must be external to the repository"
    );
    std::fs::create_dir(output).context("reserve a new preparation directory")?;
    let output = output.canonicalize()?;
    let storage_path = output.join("codestory.db");
    let defaults =
        RetrievalProcessDefaults::new(output.join("runtime"), RetrievalRuntimeDefaults::default());
    let retrieval = RuntimeRetrievalConfig::for_project_profile_with_process_defaults(
        Some(&project),
        RuntimeRetrievalProfile::Local,
        None,
        &defaults,
        &RetrievalRuntimeOverrides::default(),
    );
    let runtime = Runtime::new_with_process_config(
        RuntimeProcessConfig::new_with_retrieval_config(retrieval, SourceIndexPolicy::default()),
    );
    runtime
        .project_service()
        .open_project_summary_with_storage_path(project.clone(), storage_path.clone())
        .map_err(|error| anyhow::anyhow!(error.message))?;
    runtime
        .index_service()
        .run_indexing_blocking_without_runtime_refresh(codestory_contracts::api::IndexMode::Full)
        .map_err(|error| anyhow::anyhow!(error.message))?;
    drop(runtime);
    let pin = CoreReadSession::pin(&storage_path)?;
    let lexical_root = output.join("lexical");
    let lexical_input_hash = codestory_retrieval::benchmark_support::prepare_witness_lexical_shard(
        &project,
        &pin,
        &lexical_root,
    )?;
    let prepared = Prepared {
        contract: "codestory.witness-preparation/v1".into(),
        project_root: project.clone(),
        storage_path,
        lexical_root,
        lexical_input_hash,
        publication: PacketCompilationPublicationV1 {
            project_id: codestory_workspace::project_identity_v3(&project).project_id,
            core_generation_id: pin.identity().generation_id.clone(),
            retrieval_generation: None,
        },
        core_pointer: pin.pointer().clone(),
    };
    let path = output.join("prepared.json");
    write_receipt(&path, &serde_json::to_vec_pretty(&prepared)?)?;
    println!("{}  {}", file_digest(&path)?, path.display());
    Ok(())
}

fn capture(
    prepared_path: &Path,
    expected: &str,
    case_id: String,
    phrasing_id: String,
    question: &str,
    output: &Path,
) -> Result<()> {
    ensure!(!output.exists(), "frozen capture already exists");
    ensure!(!question.trim().is_empty(), "question is required");
    let bytes = std::fs::read(prepared_path)?;
    ensure!(
        format!("{:x}", Sha256::digest(&bytes)) == expected,
        "preparation digest mismatch"
    );
    let prepared: Prepared = serde_json::from_slice(&bytes)?;
    ensure!(
        prepared.contract == "codestory.witness-preparation/v1",
        "unexpected preparation contract"
    );
    let pin = CoreReadSession::pin(&prepared.storage_path)?;
    ensure!(
        pin.pointer() == &prepared.core_pointer,
        "prepared core publication changed"
    );
    let layout = codestory_retrieval::SidecarLayout {
        lexical_data_dir: prepared.lexical_root,
        semantic_data_dir: PathBuf::new(),
        scip_artifacts_root: PathBuf::new(),
        state_file: PathBuf::new(),
    };
    let hits = codestory_retrieval::LexicalClient::new(&layout).search(
        &layout,
        &pin.identity().generation_id,
        &prepared.lexical_input_hash,
        question,
        16,
    )?;
    let descriptors = freeze_witness_descriptors(&pin, &prepared.project_root, &hits)?;
    let manifest = Manifest {
        contract: "codestory.witness-seam-input/v1".into(),
        case_id,
        phrasing_id,
        project_root: prepared.project_root,
        storage_path: prepared.storage_path,
        publication: prepared.publication,
        descriptors,
        capture: Some(json!({
            "question_sha256": format!("{:x}", Sha256::digest(question.as_bytes())),
            "query_ordinal": 0, "prepared_sha256": expected,
            "lexical_input_hash": prepared.lexical_input_hash,
            "raw_hits_sha256": format!("{:x}", Sha256::digest(serde_json::to_vec(&hits)?)),
            "candidate_count": hits.len(), "candidate_limit": 16,
            "semantic": false, "graph": false,
            "scores": hits.iter().map(|hit| hit.score).collect::<Vec<_>>(),
            "build_commit": build_provenance::SOURCE_COMMIT.trim(),
            "binary_sha256": file_digest(&std::env::current_exe()?)?,
        })),
    };
    write_receipt(output, &serde_json::to_vec_pretty(&manifest)?)?;
    println!("{}  {}", file_digest(output)?, output.display());
    Ok(())
}

fn replay(manifest_path: &Path, expected: &str, output: &Path) -> Result<()> {
    ensure!(!output.exists(), "receipt already exists");
    let manifest = read_manifest(manifest_path, expected)?;
    let pin = CoreReadSession::pin(&manifest.storage_path)?;
    let pair = run_witness_seam(
        &pin,
        &manifest.project_root,
        &manifest.publication,
        &manifest.descriptors,
    )?;
    let receipt = json!({
        "contract": "codestory.witness-seam-receipt/v1",
        "case_id": manifest.case_id,
        "phrasing_id": manifest.phrasing_id,
        "manifest_sha256": expected,
        "descriptors_sha256": pair.descriptors_sha256,
        "core_pointer": pin.pointer(),
        "build": {
            "source_commit": build_provenance::SOURCE_COMMIT.trim(),
            "source_tree": build_provenance::SOURCE_TREE.trim(),
            "profile": build_provenance::BUILD_PROFILE.trim(),
            "rustc": build_provenance::RUSTC_VV.trim(),
            "binary_sha256": file_digest(&std::env::current_exe()?)?,
        },
        "control": {
            "input": pair.control_input,
            "support": pair.control.support,
            "continuation": pair.control.continuation,
        },
        "addressed": {
            "input": pair.addressed_input,
            "support": pair.addressed.support,
            "continuation": pair.addressed.continuation,
        },
        "packet_decision": "not_evaluated",
    });
    let bytes = serde_json::to_vec_pretty(&receipt)?;
    write_receipt(output, &bytes)?;
    println!("{}  {}", file_digest(output)?, output.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn altered_or_unidentified_manifests_cannot_replay() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("input.json");
        let input = json!({
            "contract": "codestory.witness-seam-input/v1", "case_id": "case-a",
            "phrasing_id": "original", "project_root": temp.path(),
            "storage_path": temp.path().join("codestory.db"),
            "publication": {"project_id": "p", "core_generation_id": "g", "retrieval_generation": null},
            "descriptors": [],
        });
        let bytes = serde_json::to_vec(&input).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        let digest = file_digest(&path).unwrap();
        assert!(read_manifest(&path, &digest).is_ok());
        std::fs::write(&path, [bytes.as_slice(), b" "].concat()).unwrap();
        assert!(read_manifest(&path, &digest).is_err());
        let mut invalid = input;
        invalid["contract"] = json!("another-experiment");
        std::fs::write(&path, serde_json::to_vec(&invalid).unwrap()).unwrap();
        assert!(read_manifest(&path, &file_digest(&path).unwrap()).is_err());
    }

    #[test]
    fn receipt_is_exclusive_and_preserves_the_first_result() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("receipt.json");
        write_receipt(&path, b"first").unwrap();
        assert!(write_receipt(&path, b"second").is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"first");
    }

    #[test]
    fn isolated_capture_preserves_existing_lexical_addresses() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        std::fs::create_dir(&project).unwrap();
        for ordinal in 0..16 {
            std::fs::write(
                project.join(format!("unit_{ordinal}.rs")),
                format!(
                    "{}fn needle_{ordinal}() {{ let value = 1; }}\n",
                    "// unrelated header\n".repeat(80)
                ),
            )
            .unwrap();
        }
        let output = temp.path().join("prepared");
        prepare(&project, &output).unwrap();
        let preparation = output.join("prepared.json");
        let captured = temp.path().join("capture.json");
        capture(
            &preparation,
            &file_digest(&preparation).unwrap(),
            "synthetic".into(),
            "original".into(),
            "needle",
            &captured,
        )
        .unwrap();
        let manifest = read_manifest(&captured, &file_digest(&captured).unwrap()).unwrap();
        assert_eq!(manifest.descriptors.len(), 16);
        assert!(manifest.descriptors.iter().all(|candidate| matches!(
            candidate.anchor,
            codestory_contracts::evidence_address::EvidenceAnchorV1::Match { .. }
                | codestory_contracts::evidence_address::EvidenceAnchorV1::IndexedNode { .. }
        )));
        let pin = CoreReadSession::pin(&manifest.storage_path).unwrap();
        let pair = run_witness_seam(
            &pin,
            &manifest.project_root,
            &manifest.publication,
            &manifest.descriptors,
        )
        .unwrap();
        assert_eq!(
            pair.control_input.admissions,
            pair.addressed_input.admissions
        );
        assert_eq!(
            pair.control_input.sources.len(),
            pair.addressed_input.sources.len()
        );
        assert!(
            pair.addressed_input
                .sources
                .iter()
                .all(|source| source.source.contains("needle"))
        );
        assert!(!output.join("runtime/models").exists());
        // A path result remains unaddressed rather than inventing a lexical match.
        let unaddressed = temp.path().join("path-capture.json");
        capture(
            &preparation,
            &file_digest(&preparation).unwrap(),
            "synthetic".into(),
            "path".into(),
            "unit_0.rs",
            &unaddressed,
        )
        .unwrap();
        let manifest = read_manifest(&unaddressed, &file_digest(&unaddressed).unwrap()).unwrap();
        assert!(manifest.descriptors.iter().any(|candidate| matches!(
            candidate.anchor,
            codestory_contracts::evidence_address::EvidenceAnchorV1::PathOnly { .. }
        )));
    }
}
