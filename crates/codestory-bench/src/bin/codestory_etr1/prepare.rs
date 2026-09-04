use super::contract::*;
use anyhow::{Context, Result, bail, ensure};
use codestory_retrieval::benchmark_support::{Etr1LexicalIndex, Etr1LexicalMatch};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Deserialize)]
struct SyntaxDiagnostic {
    #[serde(rename = "selectedFreeze")]
    selected_freeze: ExternalBinding,
}

#[derive(Debug, Clone, Deserialize)]
struct ExternalBinding {
    path: PathBuf,
    sha256: String,
    bytes: u64,
}

#[derive(Debug, Deserialize)]
struct SelectedFreeze {
    #[serde(rename = "inventoryRows")]
    inventory_rows: Vec<SelectedRepository>,
}

#[derive(Debug, Deserialize)]
struct SelectedRepository {
    repository_id: String,
    project_root: PathBuf,
    prepared: ExternalBinding,
    units: UnitBindings,
}

#[derive(Debug, Deserialize)]
struct UnitBindings {
    line: ExternalBinding,
}

#[derive(Debug, Deserialize)]
struct PriorPreparation {
    publication: Value,
}

#[derive(Debug, Clone, Deserialize)]
struct LineUnit {
    start_line: u32,
    end_line: u32,
    byte_range: ByteRangeV1,
    content: String,
    available: bool,
    snippet: String,
    path: String,
    content_digest: String,
}

#[derive(Debug, Deserialize)]
struct Questions {
    contract: String,
    authority: String,
    repositories: Vec<QuestionRepository>,
    cases: Vec<QuestionCase>,
}

#[derive(Debug, Deserialize)]
struct QuestionRepository {
    id: String,
    commit: String,
    local_root: PathBuf,
}

#[derive(Debug, Deserialize)]
struct QuestionCase {
    case_id: String,
    repository_id: String,
    group: String,
    question: String,
    paraphrases: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct MembershipFreeze {
    method: ExternalBinding,
    records: Vec<MembershipRecord>,
}

#[derive(Debug, Clone, Deserialize)]
struct MembershipRecord {
    case_id: String,
    phrasing_id: String,
    repository_id: String,
    group: String,
    path: PathBuf,
    sha256: String,
    bytes: u64,
}

#[derive(Debug, Deserialize)]
struct MembershipFile {
    query: String,
    method_sha256: String,
    arms: BTreeMap<String, MembershipArm>,
}

#[derive(Debug, Deserialize)]
struct MembershipArm {
    terms: Vec<String>,
    matches: Vec<MembershipMatch>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct MembershipMatch {
    rowid: usize,
    score: f64,
}

#[derive(Debug, Deserialize)]
struct ModelContract {
    schema_version: u32,
    model: ModelIdentity,
    embedding: EmbeddingIdentity,
    tokenizer_config: TokenizerIdentity,
}

#[derive(Debug, Deserialize)]
struct ModelIdentity {
    sha256: String,
}

#[derive(Debug, Deserialize)]
struct EmbeddingIdentity {
    dimension: usize,
    query_prefix: String,
    document_prefix: String,
    pooling: String,
    normalization: String,
}

#[derive(Debug, Deserialize)]
struct TokenizerIdentity {
    tokenizer_sha256: String,
}

#[derive(Debug, Serialize)]
struct PublicSourceRow<'a> {
    kind: &'static str,
    path: &'a str,
    start_line: u32,
    end_line: u32,
    snippet: &'a str,
    content_digest: &'a str,
    byte_range: ByteRangeV1,
}

fn external_binding(value: &ExternalBinding) -> Result<FileBinding> {
    let binding = bind_file(&value.path, Some(&value.sha256))?;
    ensure!(
        binding.bytes == value.bytes,
        "external_binding_length_mismatch"
    );
    Ok(binding)
}

pub(super) fn git_head(root: &Path) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "HEAD^{commit}"])
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()?;
    ensure!(output.status.success(), "repository_commit_unavailable");
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

fn line_number(bytes: &[u8], offset: usize) -> u32 {
    u32::try_from(
        bytes[..offset]
            .iter()
            .filter(|byte| **byte == b'\n')
            .count()
            + 1,
    )
    .unwrap_or(u32::MAX)
}

fn end_line_number(bytes: &[u8], end: usize) -> u32 {
    line_number(bytes, end.saturating_sub(1))
}

fn render_snippet(source: &str, start_line: u32) -> String {
    let mut snippet = String::from("```text\n");
    for (offset, line) in source.split_inclusive('\n').enumerate() {
        let line = line.trim_end_matches(['\r', '\n']);
        snippet.push_str(&format!(" {:>5} | {line}\n", start_line as usize + offset));
    }
    snippet.push_str("```");
    snippet
}

fn authenticate_units(
    repository: &SelectedRepository,
    project_id: &str,
    units: &[LineUnit],
) -> Result<Vec<FrozenFragmentV1>> {
    let mut sources = HashMap::<String, Vec<u8>>::new();
    let mut result = Vec::with_capacity(units.len());
    for unit in units {
        ensure!(unit.available, "frozen_fragment_unavailable");
        if !sources.contains_key(&unit.path) {
            let path = confined_source_path(&repository.project_root, &unit.path)?;
            let bytes = std::fs::read(path)?;
            ensure!(
                sha256(&bytes) == unit.content_digest,
                "fragment_file_digest_mismatch"
            );
            sources.insert(unit.path.clone(), bytes);
        }
        let source = &sources[&unit.path];
        let start = usize::try_from(unit.byte_range.start)?;
        let end = usize::try_from(unit.byte_range.end)?;
        ensure!(
            start < end && end <= source.len(),
            "fragment_byte_range_invalid"
        );
        ensure!(
            std::str::from_utf8(&source[..start]).is_ok(),
            "fragment_start_splits_utf8"
        );
        ensure!(
            std::str::from_utf8(&source[..end]).is_ok(),
            "fragment_end_splits_utf8"
        );
        let observed = std::str::from_utf8(&source[start..end])?;
        ensure!(observed == unit.content, "fragment_source_mismatch");
        ensure!(
            line_number(source, start) == unit.start_line,
            "fragment_start_line_mismatch"
        );
        ensure!(
            end_line_number(source, end) == unit.end_line,
            "fragment_end_line_mismatch"
        );
        ensure!(
            render_snippet(observed, unit.start_line) == unit.snippet,
            "fragment_snippet_mismatch"
        );
        let row = PublicSourceRow {
            kind: "source_range",
            path: &unit.path,
            start_line: unit.start_line,
            end_line: unit.end_line,
            snippet: &unit.snippet,
            content_digest: &unit.content_digest,
            byte_range: unit.byte_range,
        };
        let serialized_row_bytes = u32::try_from(serde_json::to_vec(&row)?.len())?;
        result.push(FrozenFragmentV1 {
            fragment_id: fragment_id(
                project_id,
                &unit.path,
                &unit.content_digest,
                unit.byte_range,
            ),
            project_id: project_id.to_string(),
            path: unit.path.clone(),
            content_digest: unit.content_digest.clone(),
            byte_range: unit.byte_range,
            line_range: LineRangeV1 {
                start: unit.start_line,
                end: unit.end_line,
            },
            source: unit.content.clone(),
            serialized_row_bytes,
        });
    }
    ensure!(
        result
            .iter()
            .all(|fragment| !fragment.source.trim().is_empty()),
        "empty_fragment_document"
    );
    ensure!(
        result
            .iter()
            .map(|fragment| &fragment.fragment_id)
            .collect::<HashSet<_>>()
            .len()
            == result.len(),
        "duplicate_fragment_identity"
    );
    Ok(result)
}

fn compare_membership(
    observed_terms: &[String],
    observed: &[Etr1LexicalMatch],
    expected: &MembershipArm,
) -> Result<()> {
    ensure!(observed_terms == expected.terms, "bm25_terms_mismatch");
    ensure!(
        observed.len() == expected.matches.len(),
        "bm25_match_count_mismatch"
    );
    for (observed, expected) in observed.iter().zip(&expected.matches) {
        ensure!(observed.rowid == expected.rowid, "bm25_rowid_mismatch");
        ensure!(
            (observed.score - expected.score).abs() <= 1e-12,
            "bm25_score_mismatch"
        );
    }
    Ok(())
}

fn method_freeze() -> Value {
    json!({
        "contract": "codestory.etr1-method/v1",
        "authority": "visible_development_frontier_only",
        "question": "same authenticated BM25 seeds; equal second-round search and source ceilings; raw question versus raw question plus verbatim seed source",
        "fragment_identity": "sha256(domain || length-framed project_id,path,content_digest || little-endian half-open byte bounds)",
        "representation": "non-overlapping complete-line 512-byte frozen fragments; document and public source are the exact fragment text",
        "round_zero": {"lane":"content-only FTS5 BM25", "seeds":16, "underfill":"preserved"},
        "second_round": {"searches":"one per actual seed per arm", "successors_per_search":8, "cumulative_exclusion":true, "score":"normalized dot product", "tie_break":"fragment_id"},
        "limits": {"successors":MAX_SUCCESSORS, "descriptor_pool":MAX_POOL, "public_rows":PUBLIC_ROWS, "public_bytes":PUBLIC_BYTES},
        "query": {"control":"raw question", "candidate":"raw question + newline delimiter + seed source", "truncation":"none; complete trailing seed lines may be removed only after a typed model-limit rejection"},
        "graph": false, "bge": false, "symbol_documents": false, "host_queries": false,
        "packet_decision": "not_evaluated"
    })
}

pub fn execute(evidence_root: &Path, corpus_root: &Path, output: &Path) -> Result<()> {
    ensure!(
        evidence_root.is_absolute() && corpus_root.is_absolute(),
        "input_roots_must_be_absolute"
    );
    let source_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .context("source_root_missing")?;
    ensure!(
        output.is_absolute() && !output.starts_with(source_root),
        "output_must_be_external"
    );
    let build = build_identity()?;

    let diagnostic_binding = bind_file(
        &evidence_root.join("syntax-representation-v2/diagnostic.json"),
        Some(FRAGMENT_DIAGNOSTIC_SHA256),
    )?;
    let build_binding = bind_file(
        &evidence_root.join("syntax-representation-v2/build.json"),
        Some(FRAGMENT_BUILD_SHA256),
    )?;
    let membership_binding = bind_file(
        &evidence_root.join("fragment-eligibility-v1/membership-freeze.json"),
        Some(MEMBERSHIP_FREEZE_SHA256),
    )?;
    let questions_binding = bind_file(&corpus_root.join("questions.json"), Some(QUESTIONS_SHA256))?;
    let lexical_binding = bind_file(
        &source_root.join("crates/codestory-retrieval/src/lexical_index.rs"),
        Some(LEXICAL_POLICY_SHA256),
    )?;
    let model_binding = bind_file(
        &source_root.join("crates/codestory-llama-sys/model-contract.json"),
        Some(MODEL_CONTRACT_SHA256),
    )?;

    let model: ModelContract = read_bound_json(&model_binding)?;
    ensure!(
        model.schema_version == 1 && model.model.sha256 == MODEL_SHA256,
        "model_identity_mismatch"
    );
    ensure!(
        model.tokenizer_config.tokenizer_sha256 == TOKENIZER_SHA256,
        "tokenizer_identity_mismatch"
    );
    ensure!(
        model.embedding.dimension == VECTOR_DIMENSION
            && model.embedding.query_prefix == "Represent this query for searching relevant code: "
            && model.embedding.document_prefix.is_empty()
            && model.embedding.pooling == "cls"
            && model.embedding.normalization == "l2",
        "embedding_contract_mismatch"
    );

    let diagnostic: SyntaxDiagnostic = read_bound_json(&diagnostic_binding)?;
    let selected_binding = external_binding(&diagnostic.selected_freeze)?;
    let selected: SelectedFreeze = read_bound_json(&selected_binding)?;
    let questions: Questions = read_bound_json(&questions_binding)?;
    ensure!(
        questions.contract == "codestory.visible-questions"
            && questions.authority == "visible_development_only"
            && questions.cases.len() == 24,
        "question_contract_mismatch"
    );
    let membership: MembershipFreeze = read_bound_json(&membership_binding)?;
    ensure!(
        membership.records.len() == WORDING_COUNT,
        "membership_record_count_mismatch"
    );
    let membership_method = external_binding(&membership.method)?;

    let mut fragments = Vec::new();
    let mut repositories = Vec::new();
    let mut fragment_by_repository = BTreeMap::<String, Vec<FrozenFragmentV1>>::new();
    let mut indexes = BTreeMap::<String, Etr1LexicalIndex>::new();
    for selected_repository in &selected.inventory_rows {
        let question_repository = questions
            .repositories
            .iter()
            .find(|repository| repository.id == selected_repository.repository_id)
            .context("repository_question_pin_missing")?;
        ensure!(
            std::fs::canonicalize(&question_repository.local_root)?
                == std::fs::canonicalize(&selected_repository.project_root)?,
            "repository_root_mismatch"
        );
        ensure!(
            git_head(&selected_repository.project_root)? == question_repository.commit,
            "repository_commit_mismatch"
        );
        let prior_binding = external_binding(&selected_repository.prepared)?;
        let prior: PriorPreparation = read_bound_json(&prior_binding)?;
        let project_id = prior
            .publication
            .get("project_id")
            .and_then(Value::as_str)
            .context("project_id_missing")?
            .to_string();
        let units_binding = external_binding(&selected_repository.units.line)?;
        let units: Vec<LineUnit> = read_bound_json(&units_binding)?;
        let repo_fragments = authenticate_units(selected_repository, &project_id, &units)?;
        let score_order_sha256 = sha256(serde_json::to_vec(
            &repo_fragments
                .iter()
                .map(|fragment| &fragment.fragment_id)
                .collect::<Vec<_>>(),
        )?);
        let empty_packet = json!({"publication": prior.publication, "answer_sufficiency":"not_asserted", "support":[], "continuation":[]});
        let base_serialized_bytes = u32::try_from(serde_json::to_vec(&empty_packet)?.len())?;
        indexes.insert(
            selected_repository.repository_id.clone(),
            Etr1LexicalIndex::new(
                repo_fragments
                    .iter()
                    .map(|fragment| fragment.source.as_str()),
            )?,
        );
        repositories.push(PreparedRepositoryV1 {
            repository_id: selected_repository.repository_id.clone(),
            project_id,
            commit: question_repository.commit.clone(),
            local_root: selected_repository.project_root.clone(),
            publication: empty_packet["publication"].clone(),
            fragment_ids: repo_fragments
                .iter()
                .map(|fragment| fragment.fragment_id.clone())
                .collect(),
            score_order_sha256,
            base_serialized_bytes,
        });
        fragments.extend(repo_fragments.iter().cloned());
        fragment_by_repository.insert(selected_repository.repository_id.clone(), repo_fragments);
    }
    ensure!(fragments.len() == FRAGMENT_COUNT, "fragment_count_mismatch");

    let mut wordings = Vec::with_capacity(WORDING_COUNT);
    for record in &membership.records {
        let record_binding = bind_file(&record.path, Some(&record.sha256))?;
        ensure!(
            record_binding.bytes == record.bytes && !record.group.is_empty(),
            "membership_record_binding_mismatch"
        );
        let expected: MembershipFile = read_bound_json(&record_binding)?;
        ensure!(
            expected.method_sha256 == membership_method.sha256,
            "membership_method_mismatch"
        );
        let case = questions
            .cases
            .iter()
            .find(|case| case.case_id == record.case_id)
            .context("question_case_missing")?;
        ensure!(
            case.repository_id == record.repository_id
                && case.group == record.group
                && case.paraphrases.len() == 2,
            "question_case_binding_mismatch"
        );
        let phrasing_index = match record.phrasing_id.as_str() {
            "original" => 0,
            "paraphrase_1" => 1,
            "paraphrase_2" => 2,
            _ => bail!("unknown_phrasing_id"),
        };
        let question = [&case.question, &case.paraphrases[0], &case.paraphrases[1]][phrasing_index];
        ensure!(expected.query == *question, "membership_question_mismatch");
        let index = indexes
            .get(&record.repository_id)
            .context("lexical_index_missing")?;
        let (terms, observed) = index.search(question)?;
        let raw = expected.arms.get("raw").context("raw_membership_missing")?;
        compare_membership(&terms, &observed, raw)?;
        let repo_fragments = fragment_by_repository
            .get(&record.repository_id)
            .context("repository_fragments_missing")?;
        let seeds = natural_seed_prefix(&observed)
            .into_iter()
            .map(|item| repo_fragments[item.rowid - 1].fragment_id.clone())
            .collect::<Vec<_>>();
        let observed_matches = observed
            .iter()
            .map(|item| MembershipMatch {
                rowid: item.rowid,
                score: item.score,
            })
            .collect::<Vec<_>>();
        wordings.push(PreparedWordingV1 {
            case_id: record.case_id.clone(),
            phrasing_id: record.phrasing_id.clone(),
            repository_id: record.repository_id.clone(),
            group: record.group.clone(),
            question: question.clone(),
            question_sha256: sha256(question.as_bytes()),
            membership: record_binding,
            terms,
            bm25_match_count: u32::try_from(observed.len())?,
            bm25_matches_sha256: sha256(serde_json::to_vec(&observed_matches)?),
            seed_fragment_ids: seeds,
        });
    }
    ensure!(wordings.len() == WORDING_COUNT, "wording_count_mismatch");

    let mut fixed_inputs = BTreeMap::new();
    for (name, binding) in [
        ("fragment_diagnostic", diagnostic_binding),
        ("fragment_build", build_binding),
        ("selected_fragment_freeze", selected_binding),
        ("lexical_membership_freeze", membership_binding),
        ("lexical_membership_method", membership_method),
        ("questions", questions_binding),
        ("model_contract", model_binding),
        ("lexical_policy_source", lexical_binding),
    ] {
        fixed_inputs.insert(name.to_string(), binding);
    }

    let method_bytes = serialize_pretty(&method_freeze())?;
    let embedding_input = EmbeddingDiagnosticInput {
        contract: "codestory.embedding-diagnostic-input/v1".into(),
        records: fragments
            .iter()
            .map(|fragment| EmbeddingDiagnosticRecord {
                id: fragment.fragment_id.clone(),
                purpose: "document".into(),
                text: fragment.source.clone(),
            })
            .collect(),
    };
    let embedding_bytes = serialize_pretty(&embedding_input)?;
    let method_file = output.join("method-freeze.json");
    let embedding_file = output.join("fragment-embedding-input.json");
    let method = FileBinding {
        path: method_file,
        sha256: sha256(&method_bytes),
        bytes: method_bytes.len() as u64,
    };
    let embedding_binding = FileBinding {
        path: embedding_file,
        sha256: sha256(&embedding_bytes),
        bytes: embedding_bytes.len() as u64,
    };
    let preparation = Etr1PreparationV1 {
        contract: "codestory.etr1-preparation/v1".into(),
        authority: "visible_development_frontier_only".into(),
        packet_decision: "not_evaluated".into(),
        parent_head: PARENT_HEAD.into(),
        build,
        method,
        fixed_inputs,
        annotations: DeclaredBinding {
            path: corpus_root.join("reconciled.json"),
            sha256: ANNOTATIONS_SHA256.into(),
        },
        model_sha256: MODEL_SHA256.into(),
        tokenizer_sha256: TOKENIZER_SHA256.into(),
        embedding_input: embedding_binding,
        annotation_access: "not_accessed".into(),
        repositories,
        fragments,
        wordings,
    };
    let preparation_bytes = serialize_pretty(&preparation)?;
    let stage = stage_output_directory(output)?;
    write_exclusive(&stage.path().join("method-freeze.json"), &method_bytes)?;
    write_exclusive(
        &stage.path().join("fragment-embedding-input.json"),
        &embedding_bytes,
    )?;
    write_exclusive(&stage.path().join("preparation.json"), &preparation_bytes)?;
    publish_output_directory(stage, output)?;
    println!(
        "{}  {}",
        sha256(&preparation_bytes),
        output.join("preparation.json").display()
    );
    println!(
        "{}  {}",
        preparation.embedding_input.sha256,
        preparation.embedding_input.path.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snippet_and_line_contract_are_complete_line_based() {
        let source = "alpha\nβeta\n";
        assert_eq!(
            render_snippet(source, 4),
            "```text\n     4 | alpha\n     5 | βeta\n```"
        );
        assert_eq!(line_number(source.as_bytes(), 0), 1);
        assert_eq!(line_number(source.as_bytes(), 6), 2);
        assert_eq!(end_line_number(source.as_bytes(), source.len()), 2);
    }

    #[test]
    fn membership_comparison_refuses_score_or_order_drift() {
        let expected = MembershipArm {
            terms: vec!["alpha".into()],
            matches: vec![MembershipMatch {
                rowid: 1,
                score: -2.0,
            }],
        };
        assert!(
            compare_membership(
                &["alpha".into()],
                &[Etr1LexicalMatch {
                    rowid: 1,
                    score: -2.0,
                }],
                &expected,
            )
            .is_ok()
        );
        assert!(
            compare_membership(
                &["alpha".into()],
                &[Etr1LexicalMatch {
                    rowid: 2,
                    score: -2.0,
                }],
                &expected,
            )
            .is_err()
        );
    }
}
