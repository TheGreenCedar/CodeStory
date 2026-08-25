//! Executed benchmark-only multilingual proof observations.
//!
//! This module deliberately sits behind the proof-availability benchmark. It
//! materializes fixture sources and reads the parser, structural, and installed
//! proof-adapter projections back from a temporary store; it does not register
//! a product route or alter Q2.

use anyhow::{Context, Result};
use codestory_contracts::events::EventBus;
use codestory_contracts::graph::{EdgeKind, NodeId, NodeKind};
use codestory_contracts::proof_resolution::{
    CalleeForm, ProofResolutionProjection, ProofResolutionStatus, ResolutionEvidenceKind,
};
use codestory_indexer::{
    WorkspaceIndexer, build_proof_resolution_funnel, current_proof_resolution_adapter_roster,
    rematerialize_proof_resolution_projection,
};
use codestory_store::{
    IndexPublicationMode, IndexPublicationRecord, Store, seal_call_resolution_fact,
};
use codestory_workspace::{BuildMode, RefreshInfo};
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const PUBLIC_PROOF_ROUTE_DARK: bool = true;

/// Closed, temporary boundary for parser-backed languages without an installed
/// proof adapter. The observed adapter roster is compared to this list in the
/// contract test; this is not an expectation of a resolution result.
pub const MISSING_ADAPTER_ALLOWLIST: &[&str] = &["ruby", "php", "csharp", "swift", "dart", "bash"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FixtureClass {
    Supported,
    Unsupported,
    Hostile,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObservedDisposition {
    ContractProven,
    Unavailable,
    NonExact,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PinnedSelector {
    pub repository: &'static str,
    pub commit: &'static str,
    pub path: &'static str,
    pub selector: &'static str,
}

#[derive(Debug, Clone, Copy)]
pub struct DispatchFixture {
    pub language: &'static str,
    pub extension: &'static str,
    pub pinned_selector: PinnedSelector,
}

/// A source blob obtained from an exact Git commit and independently projected
/// through the parser and installed proof adapters. This is the benchmark's
/// materialization boundary: callers cannot manufacture a row from a string.
#[derive(Debug, Clone)]
pub struct MaterializedRepositorySource {
    pub repository: String,
    pub requested_commit: String,
    pub resolved_commit: String,
    pub path: PathBuf,
    pub selector: String,
    pub blob_sha256: String,
    pub parser_node_count: usize,
    pub call_edge_count: usize,
    pub adapter_available: bool,
    pub facts: Vec<ObservedResolutionFact>,
    source: String,
}

const DISPATCHES: &[DispatchFixture] = &[
    dispatch(
        "kotlin",
        "kt",
        "square/okio",
        "722c8be0043d99b7b08d169b0ae90a24c15267ff",
        "okio/src/commonMain/kotlin/okio/Buffer.kt",
        "Buffer.write",
    ),
    dispatch(
        "java",
        "java",
        "apache/commons-lang",
        "57f39420fef8413ea42f045f1bdba4864ff75a0c",
        "src/main/java/org/apache/commons/lang3/StringUtils.java",
        "StringUtils.isEmpty",
    ),
    dispatch(
        "cpp",
        "cpp",
        "fmtlib/fmt",
        "e8deaf2ec3b53ced589fce6f640061e5b32eeeaa",
        "src/format.cc",
        "vformat_to",
    ),
    dispatch(
        "c",
        "c",
        "redis/redis",
        "df63a65d4d4ee33ae67e9f101885074febe0bccb",
        "src/server.c",
        "main",
    ),
    dispatch(
        "javascript",
        "js",
        "expressjs/express",
        "dae209ae6559c29cfca2a1f4414c51d89ea643d5",
        "lib/application.js",
        "app.handle",
    ),
    dispatch(
        "typescript",
        "ts",
        "vercel/swr",
        "f8d4995ac555f02a2784c8fc40bc819782c60568",
        "src/index/index.ts",
        "useSWR",
    ),
    dispatch(
        "tsx",
        "tsx",
        "vercel/swr",
        "f8d4995ac555f02a2784c8fc40bc819782c60568",
        "src/_internal/index.ts",
        "SWRConfig",
    ),
    dispatch(
        "python",
        "py",
        "psf/requests",
        "6f66281a1d6326b1c9c4ac09ca30de0fc4e6ef43",
        "src/requests/api.py",
        "request",
    ),
    dispatch(
        "rust",
        "rs",
        "BurntSushi/ripgrep",
        "82313cf95849bfe425109ad9506a52154879b1b1",
        "crates/core/main.rs",
        "main",
    ),
    dispatch(
        "go",
        "go",
        "gin-gonic/gin",
        "d75fcd4c9ab260e5225de590f1f0f8c0e0e12d11",
        "gin.go",
        "New",
    ),
    dispatch(
        "ruby",
        "rb",
        "jekyll/jekyll",
        "202df571314ba1d18e9fccd81d12aaad4a703c38",
        "lib/jekyll/site.rb",
        "Site.process",
    ),
    dispatch(
        "php",
        "php",
        "Seldaek/monolog",
        "04c3499db98d7471abd9261dc83232f8fe1a252d",
        "src/Monolog/Logger.php",
        "Logger.addRecord",
    ),
    dispatch(
        "csharp",
        "cs",
        "AutoMapper/AutoMapper",
        "b57c206dc7291821e42bdf816a5637a5c1d8cb54",
        "src/AutoMapper/Mapper.cs",
        "Mapper.Map",
    ),
    dispatch(
        "swift",
        "swift",
        "Alamofire/Alamofire",
        "7595cbcf59809f9977c5f6378500de2ad73b7ddb",
        "Source/Session.swift",
        "Session.request",
    ),
    dispatch(
        "dart",
        "dart",
        "dart-lang/http",
        "89cec60a4249ae0a0316f7a50d37ac56597f52c3",
        "pkgs/http/lib/http.dart",
        "get",
    ),
    dispatch(
        "bash",
        "sh",
        "nvm-sh/nvm",
        "7079a5d61c2b49c7d35a72006860ce5edb0fac51",
        "nvm.sh",
        "nvm",
    ),
];

const STRUCTURAL_FIXTURES: &[(&str, &str, &str)] = &[
    (
        "github_actions_workflow",
        ".github/workflows/ci.yml",
        "name: ci\non: push\njobs:\n  test:\n    runs-on: ubuntu-latest\n    steps:\n      - run: echo test\n",
    ),
    (
        "docker_compose",
        "docker-compose.yml",
        "services:\n  app:\n    image: alpine:3\n    command: [\"echo\", \"ok\"]\n",
    ),
    (
        "openapi_endpoint_schema",
        "openapi.json",
        "{\"openapi\":\"3.0.0\",\"info\":{\"title\":\"fixture\",\"version\":\"1\"},\"paths\":{\"/health\":{\"get\":{\"responses\":{\"200\":{\"description\":\"ok\"}}}}}}",
    ),
    (
        "cargo_manifest",
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    ),
    (
        "markdown",
        "docs/fixture.md",
        "# Fixture\n\nA [link](https://example.test).\n",
    ),
    (
        "yaml",
        "config.yaml",
        "service:\n  name: fixture\n  port: 8080\n",
    ),
    (
        "toml",
        "config.toml",
        "[service]\nname = \"fixture\"\nport = 8080\n",
    ),
    (
        "json",
        "config.json",
        "{\"service\":{\"name\":\"fixture\",\"port\":8080}}",
    ),
    (
        "typescript_config_jsonc",
        "tsconfig.json",
        "{\n  // fixture\n  \"compilerOptions\": { \"strict\": true }\n}",
    ),
    (
        "shell",
        "scripts/fixture.zsh",
        "fixture() {\n  echo fixture\n}\nfixture\n",
    ),
    (
        "powershell",
        "scripts/fixture.ps1",
        "function Invoke-Fixture { Write-Output 'fixture' }\nInvoke-Fixture\n",
    ),
];

const EMBEDDED_FIXTURES: &[(&str, &str, &str)] = &[
    (
        "html_embedded_script",
        "embedded/script.html",
        "<html><script>function target() {} function caller() { target(); }</script></html>",
    ),
    (
        "html_embedded_style",
        "embedded/style.html",
        "<html><style>.fixture { color: red; }</style><div class=\"fixture\"></div></html>",
    ),
    (
        "vue_template",
        "embedded/fixture.vue",
        "<template><button @click=\"target\">go</button></template><script setup lang=\"ts\">function target() {}</script><style>.fixture { color: red; }</style>",
    ),
    (
        "svelte_template",
        "embedded/fixture.svelte",
        "<script>function target() {}</script><button on:click={target}>go</button><style>.fixture { color: red; }</style>",
    ),
    (
        "astro_template",
        "embedded/fixture.astro",
        "---\nfunction target() {}\n---\n<button on:click={target}>go</button><style>.fixture { color: red; }</style>",
    ),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedResolutionFact {
    pub callee_form: CalleeForm,
    pub status: ProofResolutionStatus,
    pub evidence_kinds: Vec<ResolutionEvidenceKind>,
    pub edge_correlated: bool,
    pub proof_admitted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedLanguageCase {
    pub language: &'static str,
    pub class: FixtureClass,
    pub ordinal: u8,
    pub path: PathBuf,
    pub pinned_selector: Option<PinnedSelector>,
    pub parser_node_count: usize,
    pub call_edge_count: usize,
    pub adapter_available: bool,
    pub facts: Vec<ObservedResolutionFact>,
    pub materialized_commit: Option<String>,
    pub materialized_blob_sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolutionFunnelRow {
    pub language: &'static str,
    pub class: FixtureClass,
    pub callee_form: Option<CalleeForm>,
    pub status: Option<ProofResolutionStatus>,
    pub evidence_kinds: Vec<ResolutionEvidenceKind>,
    pub edge_correlated: bool,
    pub proof_admitted: bool,
    pub disposition: ObservedDisposition,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedAnchor {
    pub producer: String,
    pub evidence_tier: String,
    pub resolution: String,
    pub content_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedSourceRange {
    pub start_line: u32,
    pub start_col: u32,
    pub end_line: u32,
    pub end_col: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContinuityObservation {
    pub route_identity: String,
    pub path: PathBuf,
    pub node_count: usize,
    pub edge_count: usize,
    pub anchors: Vec<ObservedAnchor>,
    pub source_ranges: Vec<ObservedSourceRange>,
    pub openapi_endpoint_projection: bool,
    pub admitted_semantic_fact_count: usize,
}

#[derive(Debug, Clone)]
pub struct MultilingualObservation {
    pub cases: Vec<ObservedLanguageCase>,
    pub adapter_roster: BTreeSet<String>,
}

pub fn dispatches() -> &'static [DispatchFixture] {
    DISPATCHES
}

/// Resolve one declared source through Git before it is allowed to contribute a
/// benchmark observation. The working file must still equal the requested
/// commit's blob; otherwise a checkout-local source swap is rejected.
pub fn materialize_repository_source(
    checkout: &Path,
    language: &str,
    repository: &str,
    commit: &str,
    relative_path: &Path,
    selector: &str,
) -> Result<MaterializedRepositorySource> {
    if relative_path.is_absolute()
        || relative_path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        anyhow::bail!("materializer_path_invalid");
    }
    let revision = git_output(
        checkout,
        ["rev-parse", "--verify", &format!("{commit}^{{commit}}")],
    )
    .map_err(|_| anyhow::anyhow!("materializer_commit_missing"))?;
    let resolved_commit = revision.trim().to_owned();
    if resolved_commit != commit {
        anyhow::bail!("materializer_commit_drift");
    }
    let path_text = relative_path
        .to_str()
        .context("materializer_path_not_utf8")?;
    let diff_status = Command::new("git")
        .current_dir(checkout)
        .args(["diff", "--quiet", commit, "--", path_text])
        .status()
        .context("materializer_source_swap_check_failed")?;
    if !diff_status.success() {
        anyhow::bail!("materializer_source_swap");
    }
    let object = format!("{commit}:{path_text}");
    git_output(checkout, ["cat-file", "-e", &object])
        .map_err(|_| anyhow::anyhow!("materializer_path_missing"))?;
    let bytes = git_output_bytes(checkout, ["show", &object])
        .map_err(|_| anyhow::anyhow!("materializer_blob_missing"))?;
    let source = String::from_utf8(bytes.clone())
        .map_err(|_| anyhow::anyhow!("materializer_blob_not_text"))?;
    let blob_sha256 = format!("{:x}", Sha256::digest(&bytes));
    let projected = project_materialized_source(language, relative_path, &source, selector)?;
    Ok(MaterializedRepositorySource {
        repository: repository.to_owned(),
        requested_commit: commit.to_owned(),
        resolved_commit,
        path: relative_path.to_path_buf(),
        selector: selector.to_owned(),
        blob_sha256,
        parser_node_count: projected.parser_node_count,
        call_edge_count: projected.call_edge_count,
        adapter_available: projected.adapter_available,
        facts: projected.facts,
        source,
    })
}

/// Network-capable final-qualification boundary. Ordinary tests call the
/// checkout materializer above with temporary local repositories instead.
pub fn materialize_declared_repositories(
    checkout_root: &Path,
) -> Result<Vec<MaterializedRepositorySource>> {
    fs::create_dir_all(checkout_root)?;
    DISPATCHES
        .iter()
        .map(|dispatch| {
            let checkout = checkout_root.join(dispatch.language);
            if !checkout.exists() {
                let url = format!(
                    "https://github.com/{}.git",
                    dispatch.pinned_selector.repository
                );
                let status = Command::new("git")
                    .args(["clone", "--no-checkout", &url])
                    .arg(&checkout)
                    .status()
                    .context("materializer_clone_failed")?;
                if !status.success() {
                    anyhow::bail!("materializer_clone_failed");
                }
            }
            let selector = dispatch.pinned_selector;
            let commit = selector.commit;
            git_output(&checkout, ["fetch", "--depth=1", "origin", commit])
                .map_err(|_| anyhow::anyhow!("materializer_fetch_failed"))?;
            git_output(&checkout, ["checkout", "--detach", commit])
                .map_err(|_| anyhow::anyhow!("materializer_checkout_failed"))?;
            materialize_repository_source(
                &checkout,
                dispatch.language,
                selector.repository,
                selector.commit,
                Path::new(selector.path),
                selector.selector,
            )
        })
        .collect()
}

fn project_materialized_source(
    language: &str,
    relative_path: &Path,
    source: &str,
    selector: &str,
) -> Result<MaterializedRepositorySourceProjection> {
    let workspace = tempfile::tempdir().context("create materialized source workspace")?;
    let path = workspace.path().join(relative_path);
    write_sources(&[(path.clone(), source.to_owned())])?;
    let mut store = Store::new_in_memory()?;
    index_paths(workspace.path(), &mut store, vec![path.clone()])?;
    rematerialize_proof_resolution_projection(&mut store, &publication(1))?;
    let file_id = file_id_for(&store, &path)?;
    let nodes = store.get_nodes()?;
    let selector_observed = nodes.iter().any(|node| {
        node.file_node_id == Some(NodeId(file_id))
            && (node.serialized_name == selector
                || node.serialized_name.ends_with(&format!(".{selector}"))
                || node.qualified_name.as_deref() == Some(selector)
                || node
                    .qualified_name
                    .as_deref()
                    .is_some_and(|name| name.ends_with(&format!(".{selector}"))))
    });
    if !selector_observed {
        anyhow::bail!("materializer_symbol_missing");
    }
    verify_materialized_projection(&store, source)?;
    let case = observe_case_from_store(
        &store,
        language_to_static(language)?,
        FixtureClass::Supported,
        0,
        &path,
        None,
        observed_adapter_roster().contains(language),
        None,
        None,
    )?;
    Ok(MaterializedRepositorySourceProjection {
        parser_node_count: case.parser_node_count,
        call_edge_count: case.call_edge_count,
        adapter_available: case.adapter_available,
        facts: case.facts,
    })
}

fn verify_materialized_projection(store: &Store, source: &str) -> Result<()> {
    for fact in store.get_proof_resolution_facts()? {
        let resealed = seal_call_resolution_fact(fact.clone())?;
        if resealed.fact_id != fact.fact_id
            || resealed.provenance.evidence_sha256 != fact.provenance.evidence_sha256
        {
            anyhow::bail!("materializer_fact_seal_invalid");
        }
        let start = usize::try_from(fact.callsite.start_byte)
            .map_err(|_| anyhow::anyhow!("materializer_fact_span_invalid"))?;
        let end = usize::try_from(fact.callsite.end_byte_exclusive)
            .map_err(|_| anyhow::anyhow!("materializer_fact_span_invalid"))?;
        if end > source.len()
            || start >= end
            || !source.as_bytes()[start..end]
                .windows(fact.callsite.raw_target.len())
                .any(|window| window == fact.callsite.raw_target.as_bytes())
        {
            anyhow::bail!("materializer_fact_source_mismatch");
        }
    }
    Ok(())
}

#[derive(Debug, Clone)]
struct MaterializedRepositorySourceProjection {
    parser_node_count: usize,
    call_edge_count: usize,
    adapter_available: bool,
    facts: Vec<ObservedResolutionFact>,
}

pub fn observe_multilingual_contract() -> Result<MultilingualObservation> {
    let workspace = tempfile::tempdir().context("create multilingual fixture workspace")?;
    let mut planned = Vec::with_capacity(DISPATCHES.len() * 24);
    for dispatch in DISPATCHES {
        for ordinal in 0..24_u8 {
            let class = fixture_class(ordinal);
            let relative = format!(
                "languages/{}/{class:?}-{ordinal}.{}",
                dispatch.language, dispatch.extension
            )
            .to_lowercase();
            planned.push(PlannedLanguageCase {
                language: dispatch.language,
                class,
                ordinal,
                path: workspace.path().join(relative),
                source: source_for(dispatch.language, class, ordinal),
                pinned_selector: (class == FixtureClass::Supported)
                    .then_some(dispatch.pinned_selector),
                materialized_commit: None,
                materialized_blob_sha256: None,
            });
        }
    }
    materialize_fixture_sources(workspace.path(), &mut planned)?;
    observe_planned_language_cases(workspace.path(), planned)
}

pub fn resolution_funnel(observation: &MultilingualObservation) -> Vec<ResolutionFunnelRow> {
    observation
        .cases
        .iter()
        .flat_map(|case| {
            if case.facts.is_empty() {
                return vec![ResolutionFunnelRow {
                    language: case.language,
                    class: case.class,
                    callee_form: None,
                    status: None,
                    evidence_kinds: Vec::new(),
                    edge_correlated: false,
                    proof_admitted: false,
                    disposition: if case.adapter_available {
                        ObservedDisposition::NonExact
                    } else {
                        ObservedDisposition::Unavailable
                    },
                }];
            }
            case.facts
                .iter()
                .map(|fact| ResolutionFunnelRow {
                    language: case.language,
                    class: case.class,
                    callee_form: Some(fact.callee_form),
                    status: Some(fact.status),
                    evidence_kinds: fact.evidence_kinds.clone(),
                    edge_correlated: fact.edge_correlated,
                    proof_admitted: fact.proof_admitted,
                    disposition: if fact.proof_admitted {
                        ObservedDisposition::ContractProven
                    } else if !case.adapter_available {
                        ObservedDisposition::Unavailable
                    } else {
                        ObservedDisposition::NonExact
                    },
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

pub fn observe_structural_continuity() -> Result<Vec<ContinuityObservation>> {
    let workspace = tempfile::tempdir().context("create structural fixture workspace")?;
    let mut planned = Vec::new();
    for &(route, relative, source) in STRUCTURAL_FIXTURES.iter().chain(EMBEDDED_FIXTURES) {
        planned.push((route, workspace.path().join(relative), source.to_owned()));
    }
    observe_continuity_files(workspace.path(), planned)
}

pub fn observe_language_source(
    language: &'static str,
    source: &str,
) -> Result<ObservedLanguageCase> {
    let dispatch = dispatch_for(language)?;
    let workspace = tempfile::tempdir().context("create one-language fixture workspace")?;
    let case = PlannedLanguageCase {
        language,
        class: FixtureClass::Supported,
        ordinal: 0,
        path: workspace
            .path()
            .join(format!("single.{}", dispatch.extension)),
        source: source.to_owned(),
        pinned_selector: Some(dispatch.pinned_selector),
        materialized_commit: None,
        materialized_blob_sha256: None,
    };
    observe_planned_language_cases(workspace.path(), vec![case])
        .map(|observation| observation.cases.into_iter().next().expect("one fixture"))
}

pub fn observe_language_source_after_call_edge_removal(
    language: &'static str,
    source: &str,
) -> Result<(ObservedLanguageCase, ObservedLanguageCase)> {
    let dispatch = dispatch_for(language)?;
    let workspace = tempfile::tempdir().context("create edge-mutation fixture workspace")?;
    let path = workspace
        .path()
        .join(format!("mutation.{}", dispatch.extension));
    write_sources(&[(path.clone(), source.to_owned())])?;
    let mut store = Store::new_in_memory()?;
    index_paths(workspace.path(), &mut store, vec![path.clone()])?;
    rematerialize_proof_resolution_projection(&mut store, &publication(1))?;
    let roster = observed_adapter_roster();
    let before = observe_case_from_store(
        &store,
        language,
        FixtureClass::Supported,
        0,
        &path,
        Some(dispatch.pinned_selector),
        roster.contains(language),
        None,
        None,
    )?;
    let file_id = file_id_for(&store, &path)?;
    let edge_id = store
        .get_edges()?
        .into_iter()
        .find(|edge| edge.kind == EdgeKind::CALL && edge.file_node_id == Some(NodeId(file_id)))
        .context("fixture emitted no CALL edge to mutate")?
        .id;
    store
        .get_connection()
        .execute("DELETE FROM proof_resolution_publication", [])?;
    store
        .get_connection()
        .execute("DELETE FROM proof_resolution_fact", [])?;
    store
        .get_connection()
        .execute("DELETE FROM edge WHERE id = ?1", [edge_id.0])?;
    rematerialize_proof_resolution_projection(&mut store, &publication(2))?;
    let after = observe_case_from_store(
        &store,
        language,
        FixtureClass::Supported,
        0,
        &path,
        Some(dispatch.pinned_selector),
        roster.contains(language),
        None,
        None,
    )?;
    Ok((before, after))
}

pub fn materialized_projection_rejects_injected_fact(
    language: &'static str,
    source: &str,
) -> Result<bool> {
    let dispatch = dispatch_for(language)?;
    let workspace = tempfile::tempdir().context("create injected-fact fixture workspace")?;
    let path = workspace
        .path()
        .join(format!("injected.{}", dispatch.extension));
    write_sources(&[(path.clone(), source.to_owned())])?;
    let mut store = Store::new_in_memory()?;
    index_paths(workspace.path(), &mut store, vec![path])?;
    rematerialize_proof_resolution_projection(&mut store, &publication(1))?;
    let mut facts = store.get_proof_resolution_facts()?;
    let Some(injected) = facts.first().cloned() else {
        anyhow::bail!("materializer_injected_fact_fixture_requires_fact");
    };
    facts.push(injected);
    let projection = ProofResolutionProjection {
        adapter_roster: current_proof_resolution_adapter_roster(),
        funnel: build_proof_resolution_funnel(&facts),
        facts,
    };
    Ok(store
        .replace_proof_resolution_projection(&publication(2), &projection)
        .is_err())
}

pub fn observe_structural_source(
    route: &'static str,
    relative: &str,
    source: &str,
) -> Result<ContinuityObservation> {
    let workspace = tempfile::tempdir().context("create structural mutation fixture workspace")?;
    observe_continuity_files(
        workspace.path(),
        vec![(route, workspace.path().join(relative), source.to_owned())],
    )?
    .into_iter()
    .next()
    .context("one structural fixture")
}

pub fn valid_callee_form(language: &str, form: CalleeForm) -> bool {
    !matches!(
        (language, form),
        (
            "c" | "bash",
            CalleeForm::Constructor | CalleeForm::ExplicitReceiver | CalleeForm::ImplicitReceiver
        )
    )
}

#[derive(Debug, Clone)]
struct PlannedLanguageCase {
    language: &'static str,
    class: FixtureClass,
    ordinal: u8,
    path: PathBuf,
    source: String,
    pinned_selector: Option<PinnedSelector>,
    materialized_commit: Option<String>,
    materialized_blob_sha256: Option<String>,
}

fn materialize_fixture_sources(root: &Path, planned: &mut [PlannedLanguageCase]) -> Result<()> {
    let repositories = root.join("fixture-repositories");
    for dispatch in DISPATCHES {
        let checkout = repositories.join(dispatch.language);
        let fixtures = planned
            .iter()
            .filter(|fixture| fixture.language == dispatch.language)
            .collect::<Vec<_>>();
        for fixture in &fixtures {
            let relative = fixture_relative_path(dispatch, fixture.class, fixture.ordinal);
            write_sources(&[(checkout.join(relative), fixture.source.clone())])?;
        }
        initialize_fixture_repository(&checkout)?;
        let commit = git_output(&checkout, ["rev-parse", "HEAD"])?;
        let commit = commit.trim().to_owned();
        for fixture in planned
            .iter_mut()
            .filter(|fixture| fixture.language == dispatch.language)
        {
            let relative = fixture_relative_path(dispatch, fixture.class, fixture.ordinal);
            let materialized = materialize_repository_source(
                &checkout,
                fixture.language,
                &format!("fixture/{}", fixture.language),
                &commit,
                &relative,
                &fixture_symbol(fixture.language, fixture.ordinal),
            )?;
            fixture.source = materialized.source;
            fixture.materialized_commit = Some(materialized.resolved_commit);
            fixture.materialized_blob_sha256 = Some(materialized.blob_sha256);
        }
    }
    Ok(())
}

fn initialize_fixture_repository(checkout: &Path) -> Result<()> {
    let init = Command::new("git")
        .args(["init", "--quiet"])
        .arg(checkout)
        .status()
        .context("materializer_fixture_init_failed")?;
    if !init.success() {
        anyhow::bail!("materializer_fixture_init_failed");
    }
    git_output(checkout, ["add", "."])?;
    let commit = Command::new("git")
        .current_dir(checkout)
        .args([
            "-c",
            "user.name=CodeStory benchmark",
            "-c",
            "user.email=benchmark@example.test",
            "commit",
            "--quiet",
            "-m",
            "materialize fixtures",
        ])
        .status()
        .context("materializer_fixture_commit_failed")?;
    if !commit.success() {
        anyhow::bail!("materializer_fixture_commit_failed");
    }
    Ok(())
}

fn fixture_relative_path(dispatch: &DispatchFixture, class: FixtureClass, ordinal: u8) -> PathBuf {
    PathBuf::from(format!("fixtures/{class:?}-{ordinal}.{}", dispatch.extension).to_lowercase())
}

fn fixture_symbol(language: &str, ordinal: u8) -> String {
    if language == "python" && matches!(ordinal, 12 | 18 | 19) {
        "caller".to_owned()
    } else {
        format!("caller_{ordinal}")
    }
}

fn observe_planned_language_cases(
    root: &Path,
    planned: Vec<PlannedLanguageCase>,
) -> Result<MultilingualObservation> {
    write_sources(
        &planned
            .iter()
            .map(|fixture| (fixture.path.clone(), fixture.source.clone()))
            .collect::<Vec<_>>(),
    )?;
    let mut store = Store::new_in_memory()?;
    index_paths(
        root,
        &mut store,
        planned.iter().map(|fixture| fixture.path.clone()).collect(),
    )?;
    rematerialize_proof_resolution_projection(&mut store, &publication(1))?;
    let adapter_roster = observed_adapter_roster();
    let cases = planned
        .iter()
        .map(|fixture| {
            observe_case_from_store(
                &store,
                fixture.language,
                fixture.class,
                fixture.ordinal,
                &fixture.path,
                fixture.pinned_selector,
                adapter_roster.contains(fixture.language),
                fixture.materialized_commit.clone(),
                fixture.materialized_blob_sha256.clone(),
            )
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(MultilingualObservation {
        cases,
        adapter_roster,
    })
}

fn observe_case_from_store(
    store: &Store,
    language: &'static str,
    class: FixtureClass,
    ordinal: u8,
    path: &Path,
    pinned_selector: Option<PinnedSelector>,
    adapter_available: bool,
    materialized_commit: Option<String>,
    materialized_blob_sha256: Option<String>,
) -> Result<ObservedLanguageCase> {
    let file_id = file_id_for(store, path)?;
    let parser_node_count = store
        .get_nodes()?
        .iter()
        .filter(|node| node.file_node_id == Some(NodeId(file_id)) && node.kind != NodeKind::FILE)
        .count();
    let call_edge_count = store
        .get_edges()?
        .iter()
        .filter(|edge| edge.file_node_id == Some(NodeId(file_id)) && edge.kind == EdgeKind::CALL)
        .count();
    let mut facts = store
        .get_proof_resolution_facts()?
        .into_iter()
        .filter(|fact| fact.callsite.file_id.0 == file_id)
        .map(|fact| {
            let evidence_kinds = fact
                .evidence_chain
                .iter()
                .map(|evidence| evidence.kind())
                .collect::<Vec<_>>();
            let edge_correlated = fact.edge_id.is_some()
                && fact.raw_edge_target.is_some()
                && fact.raw_callsite_identity.is_some();
            ObservedResolutionFact {
                callee_form: fact.callsite.callee_form,
                status: fact.status,
                proof_admitted: fact.status == ProofResolutionStatus::Exact
                    && edge_correlated
                    && !evidence_kinds.is_empty()
                    && fact.lookup_domain_complete,
                evidence_kinds,
                edge_correlated,
            }
        })
        .collect::<Vec<_>>();
    facts.sort_by_key(|fact| {
        (
            format!("{:?}", fact.callee_form),
            format!("{:?}", fact.status),
        )
    });
    Ok(ObservedLanguageCase {
        language,
        class,
        ordinal,
        path: path.to_path_buf(),
        pinned_selector,
        parser_node_count,
        call_edge_count,
        adapter_available,
        facts,
        materialized_commit,
        materialized_blob_sha256,
    })
}

fn observe_continuity_files(
    root: &Path,
    planned: Vec<(&'static str, PathBuf, String)>,
) -> Result<Vec<ContinuityObservation>> {
    write_sources(
        &planned
            .iter()
            .map(|(_, path, source)| (path.clone(), source.clone()))
            .collect::<Vec<_>>(),
    )?;
    let mut store = Store::new_in_memory()?;
    index_paths(
        root,
        &mut store,
        planned.iter().map(|(_, path, _)| path.clone()).collect(),
    )?;
    rematerialize_proof_resolution_projection(&mut store, &publication(1))?;
    let nodes = store.get_nodes()?;
    let edges = store.get_edges()?;
    let files = store.get_files()?;
    let facts = store.get_proof_resolution_facts()?;
    planned
        .iter()
        .map(|(_declared_route, path, _)| {
            let file_id = file_id_for(&store, path)?;
            let language = files
                .iter()
                .find(|file| file.id == file_id)
                .context("continuity projection file missing")?
                .language
                .clone();
            let file_nodes = nodes
                .iter()
                .filter(|node| {
                    node.file_node_id == Some(NodeId(file_id)) && node.kind != NodeKind::FILE
                })
                .map(|node| node.id)
                .collect::<Vec<_>>();
            let anchors = store
                .get_structural_text_units_for_nodes(&file_nodes)?
                .into_iter()
                .map(|unit| ObservedAnchor {
                    producer: unit.producer,
                    evidence_tier: unit.evidence_tier,
                    resolution: unit.resolution,
                    content_hash: unit.content_hash,
                })
                .collect::<Vec<_>>();
            let source_ranges = nodes
                .iter()
                .filter(|node| {
                    node.file_node_id == Some(NodeId(file_id))
                        && node.kind != NodeKind::FILE
                        && node.start_line.is_some()
                        && node.start_col.is_some()
                        && node.end_line.is_some()
                        && node.end_col.is_some()
                })
                .map(|node| ObservedSourceRange {
                    start_line: node.start_line.expect("filtered source range"),
                    start_col: node.start_col.expect("filtered source range"),
                    end_line: node.end_line.expect("filtered source range"),
                    end_col: node.end_col.expect("filtered source range"),
                })
                .collect();
            let openapi_endpoint_projection =
                store.has_file_owned_openapi_endpoint_projection(file_id)?;
            let call_edge_count = edges
                .iter()
                .filter(|edge| {
                    edge.file_node_id == Some(NodeId(file_id)) && edge.kind == EdgeKind::CALL
                })
                .count();
            let admitted_semantic_fact_count = facts
                .iter()
                .filter(|fact| fact.callsite.file_id.0 == file_id)
                .filter(|fact| {
                    fact.status == ProofResolutionStatus::Exact
                        && fact.edge_id.is_some()
                        && fact.raw_edge_target.is_some()
                        && fact.raw_callsite_identity.is_some()
                        && !fact.evidence_chain.is_empty()
                        && fact.lookup_domain_complete
                })
                .count();
            Ok(ContinuityObservation {
                route_identity: continuity_route_identity(
                    &language,
                    &anchors,
                    openapi_endpoint_projection,
                    call_edge_count,
                ),
                path: path.clone(),
                node_count: file_nodes.len(),
                edge_count: edges
                    .iter()
                    .filter(|edge| edge.file_node_id == Some(NodeId(file_id)))
                    .count(),
                anchors,
                source_ranges,
                openapi_endpoint_projection,
                admitted_semantic_fact_count,
            })
        })
        .collect()
}

fn continuity_route_identity(
    language: &str,
    anchors: &[ObservedAnchor],
    openapi_endpoint_projection: bool,
    call_edge_count: usize,
) -> String {
    if openapi_endpoint_projection {
        return "openapi_endpoint_schema".to_owned();
    }
    if let Some(producer) = anchors.first().map(|anchor| anchor.producer.as_str()) {
        return producer
            .strip_prefix("structural_")
            .and_then(|value| value.strip_suffix("_collector"))
            .unwrap_or(producer)
            .to_owned();
    }
    format!(
        "parser:{language}:{}",
        if call_edge_count > 0 {
            "call"
        } else {
            "source"
        }
    )
}

fn write_sources(sources: &[(PathBuf, String)]) -> Result<()> {
    for (path, source) in sources {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, source)?;
    }
    Ok(())
}

fn index_paths(root: &Path, store: &mut Store, paths: Vec<PathBuf>) -> Result<()> {
    WorkspaceIndexer::new(root.to_path_buf()).run_incremental(
        store,
        &RefreshInfo {
            mode: BuildMode::Incremental,
            files_to_index: paths,
            files_to_remove: Vec::new(),
            existing_file_ids: HashMap::new(),
        },
        &EventBus::new(),
        None,
    )?;
    Ok(())
}

fn git_output<const N: usize>(checkout: &Path, args: [&str; N]) -> Result<String> {
    let output = Command::new("git")
        .current_dir(checkout)
        .args(args)
        .output()
        .context("materializer_git_failed")?;
    if !output.status.success() {
        anyhow::bail!("materializer_git_failed");
    }
    String::from_utf8(output.stdout).context("materializer_git_output_not_text")
}

fn git_output_bytes<const N: usize>(checkout: &Path, args: [&str; N]) -> Result<Vec<u8>> {
    let output = Command::new("git")
        .current_dir(checkout)
        .args(args)
        .output()
        .context("materializer_git_failed")?;
    if !output.status.success() {
        anyhow::bail!("materializer_git_failed");
    }
    Ok(output.stdout)
}

fn file_id_for(store: &Store, path: &Path) -> Result<i64> {
    store
        .get_files()?
        .into_iter()
        .find(|file| file.path == path)
        .map(|file| file.id)
        .with_context(|| format!("missing indexed file {}", path.display()))
}

fn observed_adapter_roster() -> BTreeSet<String> {
    current_proof_resolution_adapter_roster()
        .into_iter()
        .map(|adapter| adapter.language)
        .collect()
}

fn publication(generation: u64) -> IndexPublicationRecord {
    IndexPublicationRecord {
        generation,
        generation_id: format!("multilingual-{generation}"),
        run_id: format!("multilingual-run-{generation}"),
        mode: IndexPublicationMode::Incremental,
        published_at_epoch_ms: generation as i64,
    }
}

fn fixture_class(ordinal: u8) -> FixtureClass {
    match ordinal {
        0..=11 => FixtureClass::Supported,
        12..=17 => FixtureClass::Unsupported,
        _ => FixtureClass::Hostile,
    }
}

fn dispatch_for(language: &str) -> Result<&'static DispatchFixture> {
    DISPATCHES
        .iter()
        .find(|dispatch| dispatch.language == language)
        .with_context(|| format!("unknown parser dispatch {language}"))
}

fn language_to_static(language: &str) -> Result<&'static str> {
    dispatch_for(language).map(|dispatch| dispatch.language)
}

const fn dispatch(
    language: &'static str,
    extension: &'static str,
    repository: &'static str,
    commit: &'static str,
    path: &'static str,
    selector: &'static str,
) -> DispatchFixture {
    DispatchFixture {
        language,
        extension,
        pinned_selector: PinnedSelector {
            repository,
            commit,
            path,
            selector,
        },
    }
}

fn source_for(language: &str, class: FixtureClass, ordinal: u8) -> String {
    if language == "python" && ordinal == 12 {
        return "def caller():\n    missing()\n".to_owned();
    }
    if language == "python" && ordinal == 18 {
        return "def caller():\n    target()\n\ndef target(): pass\ndef target(): pass\n"
            .to_owned();
    }
    if language == "python" && ordinal == 19 {
        return "def caller(:\n    target()\n".to_owned();
    }
    let target = format!("target_{ordinal}");
    let caller = format!("caller_{ordinal}");
    match class {
        FixtureClass::Supported => direct_call_source(language, &target, &caller),
        FixtureClass::Unsupported => missing_call_source(language, &caller, ordinal),
        FixtureClass::Hostile => hostile_call_source(language, &target, &caller),
    }
}

fn direct_call_source(language: &str, target: &str, caller: &str) -> String {
    match language {
        "kotlin" => format!("fun {target}() {{}}\nfun {caller}() {{ {target}() }}\n"),
        "java" => format!(
            "class Fixture {{\n  static void {target}() {{}}\n  static void {caller}() {{ {target}(); }}\n}}\n"
        ),
        "cpp" => format!("void {target}() {{}}\nvoid {caller}() {{ {target}(); }}\n"),
        "c" => format!("void {target}(void) {{}}\nvoid {caller}(void) {{ {target}(); }}\n"),
        "javascript" => format!(
            "export const {target} = value => value;\nexport function {caller}() {{ {target}(1); }}\n"
        ),
        "typescript" => format!(
            "export const {target} = (value: number) => value;\nexport const {caller} = () => {{ {target}(1); }};\n"
        ),
        "tsx" => format!(
            "export const {target} = (value: number) => value;\nexport function {caller}() {{ const view = <div />; {target}(1); return view; }}\n"
        ),
        "python" => format!("def {target}():\n    pass\n\ndef {caller}():\n    {target}()\n"),
        "rust" => format!("fn {target}() {{}}\nfn {caller}() {{ {target}(); }}\n"),
        "go" => {
            format!("package fixture\nfunc {target}() {{}}\nfunc {caller}() {{ {target}() }}\n")
        }
        "ruby" => format!("def {target}\nend\ndef {caller}\n  {target}()\nend\n"),
        "php" => format!(
            "<?php\nclass Fixture {{ function {target}(): void {{}} function {caller}(): void {{ $this->{target}(); }} }}\n"
        ),
        "csharp" => format!(
            "class Fixture {{ static void {target}() {{}} static void {caller}() {{ {target}(); }} }}\n"
        ),
        "swift" => format!("func {target}() {{}}\nfunc {caller}() {{ {target}() }}\n"),
        "dart" => format!("void {target}() {{}}\nvoid {caller}() {{ {target}(); }}\n"),
        "bash" => format!("{target}() {{ :; }}\n{caller}() {{ {target}; }}\n"),
        _ => unreachable!("closed parser dispatch"),
    }
}

fn missing_call_source(language: &str, caller: &str, ordinal: u8) -> String {
    let missing = format!("missing_{ordinal}");
    match language {
        "kotlin" => format!("fun {caller}() {{ {missing}() }}\n"),
        "java" => format!("class Fixture {{ static void {caller}() {{ {missing}(); }} }}\n"),
        "cpp" => format!("void {caller}() {{ {missing}(); }}\n"),
        "c" => format!("void {caller}(void) {{ {missing}(); }}\n"),
        "javascript" => format!("function {caller}() {{ {missing}(); }}\n"),
        "typescript" => format!("function {caller}(): void {{ {missing}(); }}\n"),
        "tsx" => {
            format!("export function {caller}(): JSX.Element {{ {missing}(); return <div />; }}\n")
        }
        "python" => format!("def {caller}():\n    {missing}()\n"),
        "rust" => format!("fn {caller}() {{ {missing}(); }}\n"),
        "go" => format!("package fixture\nfunc {caller}() {{ {missing}() }}\n"),
        "ruby" => format!("def {caller}\n  {missing}()\nend\n"),
        "php" => format!(
            "<?php\nclass Fixture {{ function {caller}(): void {{ $this->{missing}(); }} }}\n"
        ),
        "csharp" => format!("class Fixture {{ static void {caller}() {{ {missing}(); }} }}\n"),
        "swift" => format!("func {caller}() {{ {missing}() }}\n"),
        "dart" => format!("void {caller}() {{ {missing}(); }}\n"),
        "bash" => format!("{caller}() {{ {missing}; }}\n"),
        _ => unreachable!("closed parser dispatch"),
    }
}

fn hostile_call_source(language: &str, target: &str, caller: &str) -> String {
    match language {
        "kotlin" => format!(
            "fun {target}() {{}}\nfun {caller}() {{ val callback = ::{target}; callback() }}\n"
        ),
        "java" => format!(
            "class Fixture {{ static void {target}() {{}} static void {caller}() {{ Runnable callback = Fixture::{target}; callback.run(); }} }}\n"
        ),
        "cpp" => format!(
            "void {target}() {{}}\nvoid {caller}() {{ auto callback = {target}; callback(); }}\n"
        ),
        "c" => format!(
            "void {target}(void) {{}}\nvoid {caller}(void) {{ void (*callback)(void) = {target}; callback(); }}\n"
        ),
        "javascript" => format!(
            "function {target}() {{}}\nfunction {caller}() {{ const callback = {target}; callback(); }}\n"
        ),
        "typescript" => format!(
            "function {target}(): void {{}}\nfunction {caller}(): void {{ const callback = {target}; callback(); }}\n"
        ),
        "tsx" => format!(
            "function {target}(): void {{}}\nexport function {caller}(): JSX.Element {{ const callback = {target}; callback(); return <div />; }}\n"
        ),
        "python" => format!(
            "def {target}():\n    pass\n\ndef {caller}():\n    callback = {target}\n    callback()\n"
        ),
        "rust" => format!(
            "fn {target}() {{}}\nfn {caller}() {{ let callback = {target}; callback(); }}\n"
        ),
        "go" => format!(
            "package fixture\nfunc {target}() {{}}\nfunc {caller}() {{ callback := {target}; callback() }}\n"
        ),
        "ruby" => format!(
            "def {target}\nend\ndef {caller}\n  callback = method(:{target})\n  callback.call()\nend\n"
        ),
        "php" => format!(
            "<?php\nclass Fixture {{ function {target}(): void {{}} function {caller}(): void {{ $callback = '{target}'; $this->$callback(); $this->{target}(); }} }}\n"
        ),
        "csharp" => format!(
            "class Fixture {{ static void {target}() {{}} static void {caller}() {{ var callback = {target}; callback(); }} }}\n"
        ),
        "swift" => format!(
            "func {target}() {{}}\nfunc {caller}() {{ let callback = {target}; callback() }}\n"
        ),
        "dart" => format!(
            "void {target}() {{}}\nvoid {caller}() {{ final callback = {target}; callback(); }}\n"
        ),
        "bash" => {
            format!("{target}() {{ :; }}\n{caller}() {{ callback={target}; \"$callback\"; }}\n")
        }
        _ => unreachable!("closed parser dispatch"),
    }
}
