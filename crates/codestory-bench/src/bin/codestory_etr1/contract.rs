use crate::build_provenance;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

pub const PARENT_HEAD: &str = "c9c935d87129a79f326b650bbf23d73191df8b4f";
pub const FRAGMENT_DIAGNOSTIC_SHA256: &str =
    "ca185ed13c635bbb4b64cc6760c5025799700359ebcc4dd3bcc53e34f8cf9194";
pub const FRAGMENT_BUILD_SHA256: &str =
    "2201780e1a752db4bfcceb047bf5cd0b5a854733c4050330ef87575b960f3baf";
pub const MEMBERSHIP_FREEZE_SHA256: &str =
    "e6867b5c79706160021ec5edf60792273345ca97cae8377e69934d5e2c9992ee";
pub const QUESTIONS_SHA256: &str =
    "8e7219a59c973c02f8ea93120bb680da46a75b8272153986c76e55bfb73ca3b6";
pub const ANNOTATIONS_SHA256: &str =
    "52b0cc223292bc70f1e4fa3f52b67bf42a91e4d4b9ed997aa12c648c068e9ade";
pub const MODEL_CONTRACT_SHA256: &str =
    "cb0e3c00290f1eb21ecdcd873521d03331069b1efa766fcd1e493e6d4299b4b7";
pub const MODEL_SHA256: &str = "666db8df27c88570cdc07adca28646260038b8ca65354911d57b936ebf56efaa";
pub const TOKENIZER_SHA256: &str =
    "7465b93c945b7a266481e6785aa13e505c625562c1c046c4b762bb4da4d46082";
pub const LEXICAL_POLICY_SHA256: &str =
    "43b2478d75abd3d5689d05e08c072e4148fd21ab29bcc55533d30a494edf986b";
pub const FRAGMENT_COUNT: usize = 10_369;
pub const WORDING_COUNT: usize = 72;
pub const VECTOR_DIMENSION: usize = 768;
pub const SEED_LIMIT: usize = 16;
pub const SUCCESSORS_PER_QUERY: usize = 8;
pub const MAX_SUCCESSORS: usize = 128;
pub const MAX_POOL: usize = 144;
pub const PUBLIC_ROWS: usize = 16;
pub const PUBLIC_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ByteRangeV1 {
    pub start: u64,
    pub end: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LineRangeV1 {
    pub start: u32,
    pub end: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FrozenFragmentV1 {
    pub fragment_id: String,
    pub project_id: String,
    pub path: String,
    pub content_digest: String,
    pub byte_range: ByteRangeV1,
    pub line_range: LineRangeV1,
    pub source: String,
    pub serialized_row_bytes: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FileBinding {
    pub path: PathBuf,
    pub sha256: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DeclaredBinding {
    pub path: PathBuf,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BuildIdentity {
    pub source_commit: String,
    pub source_tree: String,
    pub source_dirty: bool,
    pub profile: String,
    pub rustc: String,
    pub binary_path: PathBuf,
    pub binary_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedRepositoryV1 {
    pub repository_id: String,
    pub project_id: String,
    pub commit: String,
    pub local_root: PathBuf,
    pub publication: Value,
    pub fragment_ids: Vec<String>,
    pub score_order_sha256: String,
    pub base_serialized_bytes: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedWordingV1 {
    pub case_id: String,
    pub phrasing_id: String,
    pub repository_id: String,
    pub group: String,
    pub question: String,
    pub question_sha256: String,
    pub membership: FileBinding,
    pub terms: Vec<String>,
    pub bm25_match_count: u32,
    pub bm25_matches_sha256: String,
    pub seed_fragment_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Etr1PreparationV1 {
    pub contract: String,
    pub authority: String,
    pub packet_decision: String,
    pub parent_head: String,
    pub build: BuildIdentity,
    pub method: FileBinding,
    pub fixed_inputs: BTreeMap<String, FileBinding>,
    pub annotations: DeclaredBinding,
    pub model_sha256: String,
    pub tokenizer_sha256: String,
    pub embedding_input: FileBinding,
    pub annotation_access: String,
    pub repositories: Vec<PreparedRepositoryV1>,
    pub fragments: Vec<FrozenFragmentV1>,
    pub wordings: Vec<PreparedWordingV1>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmbeddingDiagnosticRecord {
    pub id: String,
    pub purpose: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmbeddingDiagnosticInput {
    pub contract: String,
    pub records: Vec<EmbeddingDiagnosticRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueryReceiptV1 {
    pub query_ordinal: u32,
    pub seed_fragment_id: String,
    pub original_input_sha256: String,
    pub encoded_input_sha256: String,
    pub encoded_input: String,
    pub removed_trailing_source_lines: u32,
    pub model_limit_rejections: u32,
    pub global_batch_ordinal: u32,
    pub score_order_sha256: String,
    pub query_vector: Vec<f32>,
    pub scores: Vec<f32>,
    pub excluded_before: Vec<String>,
    pub retained_successors: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BatchReceiptV1 {
    pub global_batch_ordinal: u32,
    pub arm: String,
    pub query_ordinals: Vec<u32>,
    pub input_sha256: Vec<String>,
    pub wall_ns: u64,
    pub completed_tokens: u64,
    pub qualification_event_sequence: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct SourceAuthenticationReceiptV1 {
    pub fragment_source_bytes: u64,
    pub filesystem_bytes_read: u64,
    pub authenticated_fragment_ids: Vec<String>,
    pub file_digests: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArmTimingV1 {
    pub round_zero_bm25_ns: u64,
    pub seed_source_authentication_ns: u64,
    pub query_encoding_ns: u64,
    pub vector_search_ns: u64,
    pub descriptor_mapping_ns: u64,
    pub remaining_source_authentication_ns: u64,
    pub prepared_state_ns: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArmFrontierV1 {
    pub name: String,
    pub search_count: u32,
    pub query_receipts: Vec<QueryReceiptV1>,
    pub batch_receipts: Vec<BatchReceiptV1>,
    pub successors: Vec<String>,
    pub descriptor_pool: Vec<String>,
    pub hydrated_pool: Vec<String>,
    pub legally_selectable_pool: Vec<String>,
    pub source_authentication: SourceAuthenticationReceiptV1,
    pub token_total: u64,
    pub timing: ArmTimingV1,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Etr1WordingResultV1 {
    pub contract: String,
    pub case_id: String,
    pub phrasing_id: String,
    pub repository_id: String,
    pub group: String,
    pub question_sha256: String,
    pub seed_fragment_ids: Vec<String>,
    pub control: ArmFrontierV1,
    pub candidate: ArmFrontierV1,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Etr1RunManifestV1 {
    pub contract: String,
    pub authority: String,
    pub experiment_status: String,
    pub decision: String,
    pub parent_head: String,
    pub build: BuildIdentity,
    pub preparation: FileBinding,
    pub fragment_vectors: FileBinding,
    pub method_sha256: String,
    pub annotation_access: String,
    pub vector_artifact_loaded_before_timing: bool,
    pub initial_engine: Value,
    pub final_engine: Value,
    pub graph_invocations: u32,
    pub bge_invocations: u32,
    pub symbol_document_invocations: u32,
    pub host_query_invocations: u32,
    pub production_packet_invocations: u32,
    pub qualification_events: FileBinding,
    pub qualification_completed_token_total: u64,
    pub rows: Vec<FileBinding>,
}

pub fn sha256(bytes: impl AsRef<[u8]>) -> String {
    format!("{:x}", Sha256::digest(bytes.as_ref()))
}

pub fn digest_file(path: &Path) -> Result<String> {
    let mut file = File::open(path).with_context(|| format!("open {}", path.display()))?;
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

pub fn bind_file(path: &Path, expected: Option<&str>) -> Result<FileBinding> {
    ensure!(
        path.is_absolute(),
        "binding_path_not_absolute: {}",
        path.display()
    );
    let metadata = std::fs::symlink_metadata(path)?;
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "binding_not_regular_file"
    );
    let digest = digest_file(path)?;
    if let Some(expected) = expected {
        ensure!(
            digest == expected,
            "binding_digest_mismatch: {}",
            path.display()
        );
    }
    Ok(FileBinding {
        path: path.to_path_buf(),
        sha256: digest,
        bytes: metadata.len(),
    })
}

pub fn read_bound_json<T: DeserializeOwned>(binding: &FileBinding) -> Result<T> {
    let bytes = std::fs::read(&binding.path)?;
    ensure!(
        bytes.len() as u64 == binding.bytes,
        "binding_length_changed"
    );
    ensure!(sha256(&bytes) == binding.sha256, "binding_digest_changed");
    serde_json::from_slice(&bytes).context("parse bound JSON")
}

pub fn build_identity() -> Result<BuildIdentity> {
    let binary_path = std::fs::canonicalize(std::env::current_exe()?)?;
    Ok(BuildIdentity {
        source_commit: build_provenance::SOURCE_COMMIT.trim().to_string(),
        source_tree: build_provenance::SOURCE_TREE.trim().to_string(),
        source_dirty: build_provenance::SOURCE_DIRTY.trim() != "false",
        profile: build_provenance::BUILD_PROFILE.trim().to_string(),
        rustc: build_provenance::RUSTC_VV.trim().to_string(),
        binary_sha256: digest_file(&binary_path)?,
        binary_path,
    })
}

pub fn fragment_id(
    project_id: &str,
    path: &str,
    content_digest: &str,
    range: ByteRangeV1,
) -> String {
    let mut digest = Sha256::new();
    digest.update(b"codestory.frozen-fragment/v1\0");
    for value in [
        project_id.as_bytes(),
        path.as_bytes(),
        content_digest.as_bytes(),
    ] {
        digest.update((value.len() as u64).to_le_bytes());
        digest.update(value);
    }
    digest.update(range.start.to_le_bytes());
    digest.update(range.end.to_le_bytes());
    format!("{:x}", digest.finalize())
}

pub fn select_successors(
    score_order: &[(String, f32)],
    seeds: &BTreeSet<String>,
    prior: &BTreeSet<String>,
    limit: usize,
) -> Vec<String> {
    let mut selected = Vec::with_capacity(limit.min(score_order.len()));
    let mut seen = BTreeSet::new();
    for (fragment_id, _) in score_order {
        if !seeds.contains(fragment_id) && !prior.contains(fragment_id) && seen.insert(fragment_id)
        {
            selected.push(fragment_id.clone());
            if selected.len() == limit {
                break;
            }
        }
    }
    selected
}

#[cfg(test)]
pub fn candidate_query_with_shortening<F>(
    question: &str,
    source: &str,
    fits: F,
) -> Result<(String, u32)>
where
    F: Fn(&str) -> bool,
{
    ensure!(!question.trim().is_empty(), "question_is_empty");
    ensure!(!source.is_empty(), "seed_source_is_empty");
    let lines = source.split_inclusive('\n').collect::<Vec<_>>();
    ensure!(!lines.is_empty(), "seed_source_has_no_lines");
    for retained in (1..=lines.len()).rev() {
        let retained_source = lines[..retained].concat();
        if retained_source.trim().is_empty() {
            continue;
        }
        let query = format!("{question}\n\n{retained_source}");
        if fits(&query) {
            return Ok((query, u32::try_from(lines.len() - retained)?));
        }
    }
    anyhow::bail!("no_complete_seed_source_line_fits")
}

pub fn natural_seed_prefix<T: Clone>(matches: &[T]) -> Vec<T> {
    matches.iter().take(SEED_LIMIT).cloned().collect()
}

pub fn validate_relative_path(path: &str) -> Result<()> {
    let parsed = Path::new(path);
    ensure!(
        !path.is_empty() && !parsed.is_absolute() && !path.contains('\\'),
        "invalid_relative_path"
    );
    ensure!(
        parsed
            .components()
            .all(|component| matches!(component, Component::Normal(_))),
        "invalid_relative_path"
    );
    Ok(())
}

pub fn confined_source_path(root: &Path, relative: &str) -> Result<PathBuf> {
    validate_relative_path(relative)?;
    let root = std::fs::canonicalize(root)?;
    let candidate = std::fs::canonicalize(root.join(relative))?;
    ensure!(candidate.starts_with(&root), "source_path_escaped_root");
    Ok(candidate)
}

pub fn serialize_pretty<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    Ok(bytes)
}

pub fn write_exclusive(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

pub fn stage_output_directory(output: &Path) -> Result<tempfile::TempDir> {
    ensure!(output.is_absolute(), "output_path_not_absolute");
    ensure!(!output.exists(), "output_already_exists");
    let parent = output.parent().context("output_parent_missing")?;
    ensure!(parent.is_dir(), "output_parent_missing");
    let stage = tempfile::Builder::new()
        .prefix(".etr1-stage-")
        .tempdir_in(parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(stage.path(), std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(stage)
}

pub fn publish_output_directory(stage: tempfile::TempDir, output: &Path) -> Result<()> {
    let stage_path = stage.keep();
    if let Err(error) = std::fs::rename(&stage_path, output) {
        let _ = std::fs::remove_dir_all(&stage_path);
        return Err(error.into());
    }
    #[cfg(unix)]
    File::open(output.parent().context("output_parent_missing")?)?.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fragment_identity_binds_every_authority() {
        let range = ByteRangeV1 { start: 7, end: 19 };
        let baseline = fragment_id("project-a", "src/lib.rs", &"a".repeat(64), range);
        assert_eq!(baseline.len(), 64);
        assert_ne!(
            baseline,
            fragment_id("project-b", "src/lib.rs", &"a".repeat(64), range)
        );
        assert_ne!(
            baseline,
            fragment_id("project-a", "src/main.rs", &"a".repeat(64), range)
        );
        assert_ne!(
            baseline,
            fragment_id("project-a", "src/lib.rs", &"b".repeat(64), range)
        );
        assert_ne!(
            baseline,
            fragment_id(
                "project-a",
                "src/lib.rs",
                &"a".repeat(64),
                ByteRangeV1 { start: 8, end: 19 }
            )
        );
    }

    #[test]
    fn natural_seed_prefix_preserves_underfill_and_order() {
        assert_eq!(natural_seed_prefix(&[3, 1, 2]), vec![3, 1, 2]);
        assert_eq!(
            natural_seed_prefix(&(0..20).collect::<Vec<_>>()),
            (0..16).collect::<Vec<_>>()
        );
    }

    #[test]
    fn cumulative_exclusions_produce_unique_successors() {
        let scores = vec![
            ("seed".into(), 1.0),
            ("prior".into(), 0.9),
            ("new-a".into(), 0.8),
            ("new-b".into(), 0.8),
        ];
        assert_eq!(
            select_successors(
                &scores,
                &BTreeSet::from(["seed".into()]),
                &BTreeSet::from(["prior".into()]),
                2
            ),
            ["new-a", "new-b"]
        );
    }

    #[test]
    fn query_shortening_keeps_utf8_and_complete_lines() {
        let source = "first α line\nsecond β line\nthird γ line\n";
        let maximum = "question\n\nfirst α line\nsecond β line\n".len();
        let (query, removed) =
            candidate_query_with_shortening("question", source, |value| value.len() <= maximum)
                .unwrap();
        assert_eq!(query, "question\n\nfirst α line\nsecond β line\n");
        assert_eq!(removed, 1);
        assert!(candidate_query_with_shortening("question", source, |_| false).is_err());
    }

    #[test]
    fn relative_source_paths_reject_aliases_and_escapes() {
        assert!(validate_relative_path("src/lib.rs").is_ok());
        for path in [
            "",
            "/src/lib.rs",
            "../src/lib.rs",
            "src/../lib.rs",
            "src\\lib.rs",
        ] {
            assert!(validate_relative_path(path).is_err(), "{path}");
        }
    }

    #[test]
    fn aborted_or_cancelled_stage_never_publishes_a_partial_experiment() {
        let parent = tempfile::tempdir().unwrap();
        let output = parent.path().join("run");
        let stage = stage_output_directory(&output).unwrap();
        write_exclusive(&stage.path().join("partial.json"), b"{}\n").unwrap();
        drop(stage);
        assert!(!output.exists());
    }

    #[test]
    fn publication_is_no_clobber() {
        let parent = tempfile::tempdir().unwrap();
        let output = parent.path().join("run");
        std::fs::create_dir(&output).unwrap();
        assert!(stage_output_directory(&output).is_err());
    }
}
