//! Capture, replay and authenticate the frozen witness experiment. This hidden
//! binary never launches models or decides an evidence-quality gate.

use anyhow::{Context, Result, ensure};
use clap::{Parser, Subcommand};
use codestory_contracts::compilation::PacketCompilationPublicationV1;
use codestory_retrieval::benchmark_support::{WitnessLexicalPin, pin_witness_lexical_sources};
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
    /// Recompute the deterministic receipt against the live pinned authorities.
    ValidateReceipt {
        #[arg(long)]
        manifest: PathBuf,
        #[arg(long)]
        manifest_sha256: String,
        #[arg(long)]
        receipt: PathBuf,
        #[arg(long)]
        receipt_sha256: String,
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
    lexical_root: PathBuf,
    lexical_input_hash: String,
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
    lexical_coverage: codestory_retrieval::benchmark_support::LexicalCoverage,
    publication: PacketCompilationPublicationV1,
    core_pointer: codestory_contracts::core_publication::CorePublicationPointerV1,
    build: serde_json::Value,
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
        Command::ValidateReceipt {
            manifest,
            manifest_sha256,
            receipt,
            receipt_sha256,
        } => {
            validate_receipt(&manifest, &manifest_sha256, &receipt, &receipt_sha256)?;
            println!(
                "{}",
                json!({"contract": "codestory.witness-receipt-validation/v1",
                "manifest_sha256": manifest_sha256, "receipt_sha256": receipt_sha256,
                "build": build_identity()?})
            );
            Ok(())
        }
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
    let (lexical_input_hash, lexical_coverage) =
        codestory_retrieval::benchmark_support::prepare_witness_lexical_shard(
            &project,
            &pin,
            &lexical_root,
        )?;
    let prepared = Prepared {
        contract: "codestory.witness-preparation/v1".into(),
        project_root: project.clone(),
        storage_path,
        lexical_root,
        lexical_input_hash: lexical_input_hash.clone(),
        lexical_coverage,
        publication: PacketCompilationPublicationV1 {
            project_id: codestory_workspace::project_identity_v3(&project).project_id,
            core_generation_id: pin.identity().generation_id.clone(),
            retrieval_generation: Some(lexical_input_hash),
        },
        core_pointer: pin.pointer().clone(),
        build: build_identity()?,
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
    ensure!(
        prepared.build == build_identity()?,
        "preparation belongs to another build"
    );
    let pin = CoreReadSession::pin(&prepared.storage_path)?;
    ensure!(
        pin.pointer() == &prepared.core_pointer,
        "prepared core publication changed"
    );
    let layout = codestory_retrieval::SidecarLayout {
        lexical_data_dir: prepared.lexical_root.clone(),
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
    let lexical = pin_witness_lexical_sources(
        &prepared.lexical_root,
        &pin.identity().generation_id,
        &prepared.lexical_input_hash,
        &hits
            .iter()
            .map(|hit| hit.file_path.clone())
            .collect::<Vec<_>>(),
    )?;
    let descriptors = freeze_witness_descriptors(&pin, &lexical, &prepared.project_root, &hits)?;
    let manifest = Manifest {
        contract: "codestory.witness-seam-input/v1".into(),
        case_id,
        phrasing_id,
        project_root: prepared.project_root,
        storage_path: prepared.storage_path,
        lexical_root: prepared.lexical_root,
        lexical_input_hash: prepared.lexical_input_hash.clone(),
        publication: prepared.publication,
        descriptors,
        capture: Some(json!({
            "question_sha256": format!("{:x}", Sha256::digest(question.as_bytes())),
            "query_ordinal": 0, "prepared_sha256": expected,
            "prepared_path": prepared_path,
            "lexical_input_hash": prepared.lexical_input_hash,
            "lexical_coverage": prepared.lexical_coverage,
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
    let receipt = compute_receipt(manifest_path, expected)?;
    write_receipt(output, &serde_json::to_vec_pretty(&receipt)?)?;
    println!("{}  {}", file_digest(output)?, output.display());
    Ok(())
}

fn build_identity() -> Result<serde_json::Value> {
    Ok(json!({
        "source_commit": build_provenance::SOURCE_COMMIT.trim(),
        "source_tree": build_provenance::SOURCE_TREE.trim(),
        "profile": build_provenance::BUILD_PROFILE.trim(),
        "rustc": build_provenance::RUSTC_VV.trim(),
        "binary_sha256": file_digest(&std::env::current_exe()?)?,
    }))
}

fn compute_receipt(manifest_path: &Path, expected: &str) -> Result<serde_json::Value> {
    let manifest = read_manifest(manifest_path, expected)?;
    let capture = manifest
        .capture
        .as_ref()
        .context("capture authority missing")?;
    let preparation = Path::new(
        capture["prepared_path"]
            .as_str()
            .context("preparation path missing")?,
    );
    let prepared_bytes = std::fs::read(preparation)?;
    ensure!(
        format!("{:x}", Sha256::digest(&prepared_bytes))
            == capture["prepared_sha256"]
                .as_str()
                .context("preparation digest missing")?,
        "preparation digest changed"
    );
    let prepared: Prepared = serde_json::from_slice(&prepared_bytes)?;
    let build = build_identity()?;
    ensure!(
        prepared.contract == "codestory.witness-preparation/v1"
            && prepared.build == build
            && capture["build_commit"] == build["source_commit"]
            && capture["binary_sha256"] == build["binary_sha256"],
        "preparation, capture, and replay build identities differ"
    );
    ensure!(
        manifest.project_root == prepared.project_root
            && manifest.storage_path == prepared.storage_path
            && manifest.lexical_root == prepared.lexical_root
            && manifest.lexical_input_hash == prepared.lexical_input_hash
            && manifest.publication == prepared.publication
            && capture["lexical_input_hash"] == prepared.lexical_input_hash,
        "manifest differs from its preparation authority"
    );
    let pin = CoreReadSession::pin(&manifest.storage_path)?;
    ensure!(
        pin.pointer() == &prepared.core_pointer,
        "prepared core pointer changed"
    );
    let lexical = manifest_lexical_pin(&manifest)?;
    let pair = run_witness_seam(
        &pin,
        Some(&lexical),
        &manifest.project_root,
        &manifest.publication,
        &manifest.descriptors,
    )?;
    Ok(json!({
        "contract": "codestory.witness-seam-receipt/v1",
        "case_id": manifest.case_id,
        "phrasing_id": manifest.phrasing_id,
        "manifest_sha256": expected,
        "descriptors_sha256": pair.descriptors_sha256,
        "core_pointer": pin.pointer(),
        "build": build,
        "control": {
            "input": pair.control_input,
            "output": pair.control,
        },
        "addressed": {
            "input": pair.addressed_input,
            "output": pair.addressed,
        },
        "packet_decision": "not_evaluated",
    }))
}

fn validate_receipt(
    manifest: &Path,
    manifest_sha256: &str,
    receipt: &Path,
    receipt_sha256: &str,
) -> Result<()> {
    let bytes = std::fs::read(receipt)?;
    ensure!(
        format!("{:x}", Sha256::digest(&bytes)) == receipt_sha256,
        "receipt digest mismatch"
    );
    let observed: serde_json::Value = serde_json::from_slice(&bytes)?;
    ensure!(
        observed == compute_receipt(manifest, manifest_sha256)?,
        "receipt differs from deterministic hydration and compilation"
    );
    Ok(())
}

fn manifest_lexical_pin(manifest: &Manifest) -> Result<WitnessLexicalPin> {
    pin_witness_lexical_sources(
        &manifest.lexical_root,
        &manifest.publication.core_generation_id,
        &manifest.lexical_input_hash,
        &manifest
            .descriptors
            .iter()
            .filter_map(|descriptor| {
                descriptor
                    .path
                    .as_ref()
                    .map(|path| path.as_str().to_owned())
            })
            .collect::<Vec<_>>(),
    )
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
            "lexical_root": temp.path().join("lexical"), "lexical_input_hash": "hash",
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
        std::fs::write(project.join("image.png"), [0xff, 0xfe, 0xfd]).unwrap();
        prepare(&project, &output).unwrap();
        let preparation = output.join("prepared.json");
        let prepared: Prepared =
            serde_json::from_slice(&std::fs::read(&preparation).unwrap()).unwrap();
        assert_eq!(prepared.lexical_coverage.unreadable_files, 1);
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
            Some(
                codestory_contracts::evidence_address::EvidenceAnchorV1::Match { .. }
                    | codestory_contracts::evidence_address::EvidenceAnchorV1::IndexedNode { .. }
            )
        )));
        let pin = CoreReadSession::pin(&manifest.storage_path).unwrap();
        let lexical = manifest_lexical_pin(&manifest).unwrap();
        let pair = run_witness_seam(
            &pin,
            Some(&lexical),
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
        let manifest_digest = file_digest(&captured).unwrap();
        let receipt = temp.path().join("receipt.json");
        replay(&captured, &manifest_digest, &receipt).unwrap();
        validate_receipt(
            &captured,
            &manifest_digest,
            &receipt,
            &file_digest(&receipt).unwrap(),
        )
        .unwrap();
        let original = compute_receipt(&captured, &manifest_digest).unwrap();
        // Rehashing altered evidence must not authenticate a different operation.
        let mut mutations = Vec::new();
        let mut changed = original.clone();
        changed["addressed"]["output"]["support"] = json!([]);
        mutations.push(changed);
        let mut changed = original.clone();
        changed["addressed"]["input"]["sources"][0]["source"] =
            json!("unexposed fabricated source");
        changed["addressed"]["output"]["support"] = json!([]);
        mutations.push(changed);
        let mut changed = original.clone();
        changed["addressed"]["input"]["sources"]
            .as_array_mut()
            .unwrap()
            .remove(0);
        changed["addressed"]["input"]["admission_gaps"] = json!([{
            "kind": "source_budget_exceeded", "stable_identity": original["addressed"]["input"]["admissions"][0]["stable_identity"],
            "exact_selector_ordinal": null,
        }]);
        mutations.push(changed);
        let mut changed = original.clone();
        changed["addressed"]["output"]["support"] = json!([{
            "kind": "symbol_location", "path": "absent.rs", "symbol": "invented",
        }]);
        mutations.push(changed);
        let mut changed = original.clone();
        changed["addressed"]["output"]["continuation"] = json!(["x".repeat(17000)]);
        mutations.push(changed);
        for (index, changed) in mutations.into_iter().enumerate() {
            let path = temp.path().join(format!("altered-{index}.json"));
            write_receipt(&path, &serde_json::to_vec(&changed).unwrap()).unwrap();
            assert!(
                validate_receipt(
                    &captured,
                    &manifest_digest,
                    &path,
                    &file_digest(&path).unwrap()
                )
                .is_err()
            );
        }
        let input: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&captured).unwrap()).unwrap();
        for (index, pointer) in [
            "/publication/project_id",
            "/publication/core_generation_id",
            "/publication/retrieval_generation",
            "/lexical_input_hash",
            "/capture/lexical_input_hash",
            "/capture/build_commit",
            "/capture/binary_sha256",
            "/capture/prepared_sha256",
        ]
        .iter()
        .enumerate()
        {
            let mut changed = input.clone();
            *changed.pointer_mut(pointer).unwrap() = json!("wrong-authority");
            let path = temp.path().join(format!("authority-{index}.json"));
            write_receipt(&path, &serde_json::to_vec(&changed).unwrap()).unwrap();
            assert!(
                compute_receipt(&path, &file_digest(&path).unwrap()).is_err(),
                "accepted {pointer}"
            );
        }
        for count in [0, 1, 15] {
            let pair = run_witness_seam(
                &pin,
                Some(&lexical),
                &manifest.project_root,
                &manifest.publication,
                &manifest.descriptors[..count],
            )
            .expect("natural retrieval underfill is preserved in both arms");
            assert_eq!(pair.control_input.admissions.len(), count);
            assert_eq!(
                pair.control_input.admissions,
                pair.addressed_input.admissions
            );
        }
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
            Some(codestory_contracts::evidence_address::EvidenceAnchorV1::PathOnly { .. })
        )));
        let pair = run_witness_seam(
            &pin,
            Some(&manifest_lexical_pin(&manifest).unwrap()),
            &manifest.project_root,
            &manifest.publication,
            &manifest.descriptors,
        )
        .expect("missing source precision is an explicit gap, not a failed experiment");
        assert!(!pair.addressed_input.admission_gaps.is_empty());
        assert_eq!(
            pair.control_input.admission_gaps,
            pair.addressed_input.admission_gaps
        );
    }

    #[test]
    fn lexical_source_without_a_parser_file_is_authenticated_by_its_shard() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        std::fs::create_dir(&project).unwrap();
        std::fs::write(project.join("lib.rs"), "pub fn ordinary() {}\n").unwrap();
        std::fs::write(
            project.join("guide.rst"),
            format!("{}needle_document\n", "preamble\n".repeat(40)),
        )
        .unwrap();
        let output = temp.path().join("prepared");
        prepare(&project, &output).unwrap();
        let preparation = output.join("prepared.json");
        let captured = temp.path().join("capture.json");
        capture(
            &preparation,
            &file_digest(&preparation).unwrap(),
            "synthetic".into(),
            "original".into(),
            "needle_document",
            &captured,
        )
        .unwrap();
        let manifest = read_manifest(&captured, &file_digest(&captured).unwrap()).unwrap();
        assert_eq!(manifest.descriptors.len(), 1);
        let pin = CoreReadSession::pin(&manifest.storage_path).unwrap();
        assert!(
            pin.storage()
                .get_file_by_path(&project.join("guide.rst"))
                .unwrap()
                .is_none()
        );
        let lexical = manifest_lexical_pin(&manifest).unwrap();
        let pair = run_witness_seam(
            &pin,
            Some(&lexical),
            &manifest.project_root,
            &manifest.publication,
            &manifest.descriptors,
        )
        .unwrap();
        assert_eq!(pair.addressed_input.sources.len(), 1);
        assert!(
            pair.addressed_input.sources[0]
                .source
                .contains("needle_document")
        );
        let manifest_digest = file_digest(&captured).unwrap();
        let mut false_completeness = compute_receipt(&captured, &manifest_digest).unwrap();
        let mut false_header = false_completeness.clone();
        false_header["addressed"] = false_header["control"].clone();
        assert_ne!(
            false_header, false_completeness,
            "the lexical control must expose a different window"
        );
        let header_receipt = temp.path().join("false-header.json");
        write_receipt(&header_receipt, &serde_json::to_vec(&false_header).unwrap()).unwrap();
        assert!(
            validate_receipt(
                &captured,
                &manifest_digest,
                &header_receipt,
                &file_digest(&header_receipt).unwrap()
            )
            .is_err()
        );
        assert_eq!(
            false_completeness["addressed"]["input"]["sources"][0]["parser_completeness"],
            "unknown"
        );
        for arm in ["control", "addressed"] {
            false_completeness[arm]["input"]["sources"][0]["parser_completeness"] =
                json!("complete");
        }
        let altered = temp.path().join("false-completeness.json");
        write_receipt(&altered, &serde_json::to_vec(&false_completeness).unwrap()).unwrap();
        assert!(
            validate_receipt(
                &captured,
                &manifest_digest,
                &altered,
                &file_digest(&altered).unwrap()
            )
            .is_err()
        );
        let mut wrong = read_manifest(&captured, &file_digest(&captured).unwrap()).unwrap();
        wrong.lexical_input_hash = "0".repeat(64);
        assert!(
            manifest_lexical_pin(&wrong).is_err(),
            "a different lexical publication cannot authorize source"
        );
        std::fs::write(project.join("guide.rst"), "replaced\n").unwrap();
        assert!(
            run_witness_seam(
                &pin,
                Some(&lexical),
                &manifest.project_root,
                &manifest.publication,
                &manifest.descriptors
            )
            .is_err()
        );
        let shard = manifest
            .lexical_root
            .join("shards")
            .join(&manifest.publication.core_generation_id)
            .join("lexical-index.sqlite3");
        let mut permissions = std::fs::metadata(&shard).unwrap().permissions();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            permissions.set_mode(0o600);
        }
        #[cfg(not(unix))]
        permissions.set_readonly(false);
        std::fs::set_permissions(&shard, permissions).unwrap();
        std::fs::OpenOptions::new()
            .write(true)
            .open(shard)
            .unwrap()
            .set_len(16)
            .unwrap();
        assert!(
            manifest_lexical_pin(&manifest).is_err(),
            "component truncation invalidates its warm seal"
        );
    }
}
