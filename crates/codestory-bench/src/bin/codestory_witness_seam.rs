//! Deterministic replay of the frozen sixteen-candidate witness experiment.
//! This binary never retrieves candidates, launches models, or decides a gate.

use anyhow::{Context, Result, ensure};
use clap::Parser;
use codestory_contracts::compilation::PacketCompilationPublicationV1;
use codestory_runtime::benchmark_support::{WitnessSeamDescriptor, run_witness_seam};
use codestory_store::CoreReadSession;
use serde::Deserialize;
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
    #[arg(long)]
    manifest: PathBuf,
    #[arg(long)]
    manifest_sha256: String,
    /// A new external receipt file. An existing file is never overwritten.
    #[arg(long)]
    output: PathBuf,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    contract: String,
    case_id: String,
    phrasing_id: String,
    project_root: PathBuf,
    storage_path: PathBuf,
    publication: PacketCompilationPublicationV1,
    descriptors: Vec<WitnessSeamDescriptor>,
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
    ensure!(!args.output.exists(), "receipt already exists");
    let manifest = read_manifest(&args.manifest, &args.manifest_sha256)?;
    // A dirty binary can exercise tests, but cannot produce experiment evidence.
    ensure!(
        build_provenance::SOURCE_DIRTY.trim() == "false",
        "witness replay requires a clean-source binary"
    );
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
        "manifest_sha256": args.manifest_sha256,
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
    write_receipt(&args.output, &bytes)?;
    println!("{}  {}", file_digest(&args.output)?, args.output.display());
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
}
