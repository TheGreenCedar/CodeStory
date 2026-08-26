#[path = "../src/bin/codestory_proof_availability/multilingual_contract.rs"]
mod multilingual_contract;

use codestory_contracts::language_support::STRUCTURAL_SOURCE_PROOF_CONTRACTS;
use codestory_contracts::proof_resolution::{
    CalleeForm, ProofResolutionStatus, ResolutionEvidenceKind,
};
use multilingual_contract::{
    FixtureClass, MISSING_ADAPTER_ALLOWLIST, ObservedDisposition, PUBLIC_PROOF_ROUTE_DARK,
    dispatches, materialize_repository_source, materialized_projection_rejects_injected_fact,
    observe_language_source, observe_language_source_after_call_edge_removal,
    observe_multilingual_contract, observe_structural_continuity, observe_structural_source,
    resolution_funnel, valid_callee_form,
};
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::process::Command;

const PARSER_DISPATCHES: &[&str] = &[
    "kotlin",
    "java",
    "cpp",
    "c",
    "javascript",
    "typescript",
    "tsx",
    "python",
    "rust",
    "go",
    "ruby",
    "php",
    "csharp",
    "swift",
    "dart",
    "bash",
];

#[test]
fn all_language_rows_are_real_parser_and_adapter_observations() -> anyhow::Result<()> {
    let observation = observe_multilingual_contract()?;
    assert_eq!(observation.cases.len(), 16 * 24);
    assert_eq!(
        dispatches()
            .iter()
            .map(|dispatch| dispatch.language)
            .collect::<BTreeSet<_>>(),
        PARSER_DISPATCHES.iter().copied().collect(),
    );

    for dispatch in dispatches() {
        let cases = observation
            .cases
            .iter()
            .filter(|case| case.language == dispatch.language)
            .collect::<Vec<_>>();
        assert_eq!(
            cases
                .iter()
                .filter(|case| case.class == FixtureClass::Supported)
                .count(),
            12
        );
        assert_eq!(
            cases
                .iter()
                .filter(|case| matches!(
                    case.class,
                    FixtureClass::Unsupported | FixtureClass::Hostile
                ))
                .count(),
            12
        );
        assert!(
            cases
                .iter()
                .any(|case| case.class == FixtureClass::Unsupported)
        );
        assert!(cases.iter().any(|case| case.class == FixtureClass::Hostile));
        assert!(
            cases.iter().all(|case| case.parser_node_count > 0),
            "{} parser extraction emitted no nodes",
            dispatch.language
        );
        assert!(
            cases.iter().all(|case| case.call_edge_count > 0),
            "{} parser extraction emitted no call edges",
            dispatch.language
        );
        assert!(cases.iter().all(|case| {
            case.materialized_commit.is_some()
                && case
                    .materialized_blob_sha256
                    .as_deref()
                    .is_some_and(|digest| digest.len() == 64)
        }));

        for positive in cases
            .iter()
            .filter(|case| case.class == FixtureClass::Supported)
        {
            let selector = positive
                .pinned_selector
                .expect("positive fixture must retain its pinned corpus selector");
            assert_eq!(selector, dispatch.pinned_selector);
            assert_eq!(selector.commit.len(), 40);
            assert!(!selector.path.is_empty() && !selector.selector.is_empty());
            if positive.adapter_available {
                assert!(
                    positive.facts.iter().any(|fact| fact.proof_admitted),
                    "{} positive fixture was not proven by its installed adapter",
                    positive.path.display()
                );
            } else {
                assert!(
                    positive.facts.is_empty(),
                    "{} is missing an adapter and must not fabricate proof facts",
                    dispatch.language
                );
            }
        }
    }

    let observed_missing = dispatches()
        .iter()
        .filter(|dispatch| !observation.adapter_roster.contains(dispatch.language))
        .map(|dispatch| dispatch.language)
        .collect::<BTreeSet<_>>();
    assert_eq!(
        observed_missing,
        MISSING_ADAPTER_ALLOWLIST.iter().copied().collect()
    );
    assert_eq!(MISSING_ADAPTER_ALLOWLIST, &["bash"]);
    for case in &observation.cases {
        let components = case
            .path
            .components()
            .filter_map(|component| component.as_os_str().to_str())
            .collect::<Vec<_>>();
        match case.language {
            "swift" => assert!(
                components
                    .iter()
                    .any(|component| matches!(*component, "Source" | "Sources")),
                "Swift benchmark cohort escaped its authenticated source layout: {}",
                case.path.display()
            ),
            "dart" => assert!(
                components.contains(&"lib"),
                "Dart benchmark cohort escaped lib: {}",
                case.path.display()
            ),
            _ => {}
        }
    }
    Ok(())
}

#[test]
fn funnel_uses_canonical_types_and_observed_nonexact_states() -> anyhow::Result<()> {
    let observation = observe_multilingual_contract()?;
    let funnel = resolution_funnel(&observation);
    assert!(
        funnel.len() >= 16 * 24,
        "every fixture must produce at least one closed funnel row"
    );
    let statuses = funnel
        .iter()
        .filter_map(|row| row.status)
        .collect::<BTreeSet<_>>();
    assert!(statuses.contains(&ProofResolutionStatus::Ambiguous));
    assert!(statuses.contains(&ProofResolutionStatus::MissingBinding));
    assert!(statuses.contains(&ProofResolutionStatus::IncompleteDomain));

    for row in &funnel {
        if let Some(form) = row.callee_form {
            assert!(valid_callee_form(row.language, form));
        }
        for evidence in &row.evidence_kinds {
            assert!(matches!(
                evidence,
                ResolutionEvidenceKind::SameFileDeclaration
                    | ResolutionEvidenceKind::SamePackageDeclaration
                    | ResolutionEvidenceKind::StaticImportBinding
                    | ResolutionEvidenceKind::QualifiedPath
                    | ResolutionEvidenceKind::ExplicitReceiverType
                    | ResolutionEvidenceKind::ConstructorBinding
                    | ResolutionEvidenceKind::ImplicitReceiver
            ));
        }
        assert_eq!(
            row.disposition == ObservedDisposition::ContractProven,
            row.proof_admitted
        );
    }
    for language in ["c", "bash"] {
        for impossible in [
            CalleeForm::Constructor,
            CalleeForm::ExplicitReceiver,
            CalleeForm::ImplicitReceiver,
        ] {
            assert!(!valid_callee_form(language, impossible));
        }
    }
    assert!(valid_callee_form("rust", CalleeForm::Constructor));
    Ok(())
}

#[test]
fn structural_and_embedded_routes_snapshot_real_nodes_edges_and_anchors() -> anyhow::Result<()> {
    let observation = observe_structural_continuity()?;
    let emitted_structural = observation
        .iter()
        .filter(|row| !row.anchors.is_empty() || row.openapi_endpoint_projection)
        .map(|row| row.route_identity.as_str())
        .collect::<BTreeSet<_>>();
    let required_structural = STRUCTURAL_SOURCE_PROOF_CONTRACTS
        .iter()
        .map(|contract| contract.collector_name)
        .collect::<BTreeSet<_>>();
    assert!(required_structural.is_subset(&emitted_structural));
    assert_eq!(
        observation.len(),
        16,
        "eleven structural profiles and five embedded parser routes must execute"
    );
    assert!(
        observation
            .iter()
            .any(|row| row.route_identity.starts_with("parser:")),
        "embedded routes must be identified from parser output, not caller labels"
    );
    for row in observation {
        assert!(
            row.node_count > 0,
            "{} emitted no nodes",
            row.route_identity
        );
        assert!(
            row.edge_count > 0,
            "{} emitted no edges",
            row.route_identity
        );
        assert!(
            !row.source_ranges.is_empty(),
            "{} emitted no source ranges",
            row.route_identity
        );
        assert!(
            !row.anchors.is_empty()
                || row.openapi_endpoint_projection
                || row.route_identity.starts_with("parser:"),
            "{} emitted no anchored, parser, or dedicated OpenAPI evidence",
            row.route_identity
        );
        assert!(row.anchors.iter().all(|anchor| !anchor.producer.is_empty()
            && !anchor.evidence_tier.is_empty()
            && !anchor.resolution.is_empty()));
        assert_eq!(
            row.admitted_semantic_fact_count, 0,
            "continuity must remain source evidence only"
        );
    }
    Ok(())
}

#[test]
fn mutations_change_the_observed_call_edge_anchor_hostile_and_embedded_outputs()
-> anyhow::Result<()> {
    let source_with_call = "def target():\n    pass\n\ndef caller():\n    target()\n";
    let source_without_call = "def target():\n    pass\n\ndef caller():\n    pass\n";
    let with_call = observe_language_source("python", source_with_call)?;
    let without_call = observe_language_source("python", source_without_call)?;
    assert!(with_call.call_edge_count > without_call.call_edge_count);
    assert!(with_call.facts.iter().any(|fact| fact.proof_admitted));
    assert!(without_call.facts.iter().all(|fact| !fact.proof_admitted));

    let (before_edge_removal, after_edge_removal) =
        observe_language_source_after_call_edge_removal("python", source_with_call)?;
    assert!(before_edge_removal.call_edge_count > after_edge_removal.call_edge_count);
    assert!(
        before_edge_removal
            .facts
            .iter()
            .any(|fact| fact.proof_admitted)
    );
    assert!(
        after_edge_removal
            .facts
            .iter()
            .all(|fact| !fact.proof_admitted)
    );

    let anchor_before =
        observe_structural_source("markdown", "docs/fixture.md", "# Kept anchor\n\nbody\n")?;
    let anchor_after = observe_structural_source("markdown", "docs/fixture.md", "body\n")?;
    assert_ne!(anchor_before.anchors, anchor_after.anchors);

    let ambiguous = observe_language_source(
        "python",
        "def caller():\n    target()\n\ndef target(): pass\ndef target(): pass\n",
    )?;
    let exact = observe_language_source("python", source_with_call)?;
    assert!(
        ambiguous
            .facts
            .iter()
            .any(|fact| fact.status == ProofResolutionStatus::Ambiguous)
    );
    assert!(
        exact
            .facts
            .iter()
            .any(|fact| fact.status == ProofResolutionStatus::Exact)
    );

    let embedded = observe_structural_source(
        "vue_template",
        "embedded/fixture.vue",
        "<template><button @click=\"target\">go</button></template><script setup lang=\"ts\">function target() {}</script><style>.fixture { color: red; }</style>",
    )?;
    let removed_region = observe_structural_source(
        "vue_template",
        "embedded/fixture.vue",
        "<template><button>go</button></template>",
    )?;
    assert_ne!(
        (embedded.node_count, embedded.edge_count, embedded.anchors),
        (
            removed_region.node_count,
            removed_region.edge_count,
            removed_region.anchors
        )
    );
    Ok(())
}

#[test]
fn git_materializer_rejects_commit_path_symbol_source_swap_and_injected_fact() -> anyhow::Result<()>
{
    let repository = tempfile::tempdir()?;
    let relative = Path::new("src/fixture.py");
    let source = "def target():\n    pass\n\ndef caller():\n    target()\n";
    fs::create_dir_all(repository.path().join("src"))?;
    fs::write(repository.path().join(relative), source)?;
    assert!(
        Command::new("git")
            .args(["init", "--quiet"])
            .arg(repository.path())
            .status()?
            .success()
    );
    assert!(
        Command::new("git")
            .current_dir(repository.path())
            .args(["add", "."])
            .status()?
            .success()
    );
    assert!(
        Command::new("git")
            .current_dir(repository.path())
            .args([
                "-c",
                "user.name=CodeStory test",
                "-c",
                "user.email=test@example.test",
                "commit",
                "--quiet",
                "-m",
                "fixture",
            ])
            .status()?
            .success()
    );
    let commit = String::from_utf8(
        Command::new("git")
            .current_dir(repository.path())
            .args(["rev-parse", "HEAD"])
            .output()?
            .stdout,
    )?
    .trim()
    .to_owned();

    let materialized = materialize_repository_source(
        repository.path(),
        "python",
        "fixture/python",
        &commit,
        relative,
        "caller",
    )?;
    assert_eq!(materialized.resolved_commit, commit);
    assert_eq!(materialized.blob_sha256.len(), 64);
    assert!(materialized.parser_node_count > 0 && materialized.call_edge_count > 0);
    assert!(materialized.facts.iter().any(|fact| fact.proof_admitted));
    assert!(
        materialize_repository_source(
            repository.path(),
            "python",
            "fixture/python",
            "0000000000000000000000000000000000000000",
            relative,
            "caller",
        )
        .is_err()
    );
    assert!(
        materialize_repository_source(
            repository.path(),
            "python",
            "fixture/python",
            &commit,
            Path::new("missing.py"),
            "caller",
        )
        .is_err()
    );
    assert!(
        materialize_repository_source(
            repository.path(),
            "python",
            "fixture/python",
            &commit,
            relative,
            "missing_symbol",
        )
        .is_err()
    );

    fs::write(
        repository.path().join(relative),
        "def replacement():\n    pass\n",
    )?;
    assert!(
        materialize_repository_source(
            repository.path(),
            "python",
            "fixture/python",
            &commit,
            relative,
            "caller",
        )
        .is_err()
    );
    assert!(materialized_projection_rejects_injected_fact(
        "python", source
    )?);
    Ok(())
}

#[test]
fn contract_stays_dark_to_the_public_proof_route() {
    const { assert!(PUBLIC_PROOF_ROUTE_DARK) };
}
