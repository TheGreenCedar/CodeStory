//! Contract tests for prompt-blind repository evidence compilation.

use codestory_agent::evidence_compiler::compile_repository_evidence;
use codestory_contracts::api::SupportUnitKindDto;
use codestory_contracts::compilation::{
    PACKET_COMPILATION_CONTRACT_VERSION_V1, PUBLIC_PACKET_SERIALIZED_MAX_BYTES,
    PacketAdmissionOriginV1, PacketAdmissionReceiptV1, PacketCompilationInputV1,
    PacketCompilationPublicationV1, PacketDirectedRelationV1, PacketHydratedSourceRangeV1,
    PacketParserCompletenessV1, PacketRelationCertaintyV1, PacketRelationKindV1,
    PacketStructuralGapReasonV1,
};

fn admission(
    ordinal: u32,
    identity: &str,
    origin: PacketAdmissionOriginV1,
) -> PacketAdmissionReceiptV1 {
    PacketAdmissionReceiptV1 {
        packet_ordinal: ordinal,
        stable_identity: identity.to_string(),
        score_version: "retrieval-score/v1".into(),
        reserved_source_bytes: 512,
        origin,
    }
}

fn source(
    identity: &str,
    path: &str,
    start_line: u32,
    end_line: u32,
) -> PacketHydratedSourceRangeV1 {
    PacketHydratedSourceRangeV1 {
        stable_identity: identity.to_string(),
        path: path.to_string(),
        symbol: Some(identity.to_string()),
        start_line,
        end_line,
        source: format!("fn {}() {{}}", identity.replace([':', '-'], "_")),
        parser_completeness: PacketParserCompletenessV1::Complete,
    }
}

fn relation(
    id: &str,
    from: &str,
    to: &str,
    certainty: PacketRelationCertaintyV1,
) -> PacketDirectedRelationV1 {
    PacketDirectedRelationV1 {
        relation_id: id.to_string(),
        from_identity: from.to_string(),
        to_identity: to.to_string(),
        relation_kind: PacketRelationKindV1::Call,
        certainty,
    }
}

fn input(
    admissions: Vec<PacketAdmissionReceiptV1>,
    sources: Vec<PacketHydratedSourceRangeV1>,
    relations: Vec<PacketDirectedRelationV1>,
) -> PacketCompilationInputV1 {
    PacketCompilationInputV1 {
        contract_version: PACKET_COMPILATION_CONTRACT_VERSION_V1,
        publication: PacketCompilationPublicationV1 {
            project_id: "project".into(),
            core_generation_id: "core".into(),
            retrieval_generation: Some("retrieval".into()),
        },
        admissions,
        sources,
        relations,
        ambiguities: Vec::new(),
        admission_gaps: Vec::new(),
    }
}

#[test]
fn compiler_is_prompt_blind_and_invariant_to_input_permutation() {
    let admissions = vec![
        admission(0, "node:alpha", PacketAdmissionOriginV1::ExactTypedSelector),
        admission(1, "node:omega", PacketAdmissionOriginV1::Retrieval),
    ];
    let sources = vec![
        source("node:alpha", "src/alpha.rs", 1, 3),
        source("node:omega", "src/omega.rs", 5, 7),
    ];
    let relations = vec![relation(
        "edge-1",
        "node:alpha",
        "node:omega",
        PacketRelationCertaintyV1::Certain,
    )];
    let baseline = input(admissions.clone(), sources.clone(), relations.clone());
    let mut permuted = input(admissions, sources, relations);
    permuted.admissions.reverse();
    permuted.sources.reverse();
    permuted.relations.reverse();

    assert_eq!(
        compile_repository_evidence(&baseline),
        compile_repository_evidence(&permuted)
    );
    let serialized = serde_json::to_value(baseline).expect("serialize compiler input");
    for forbidden in [
        "question",
        "prompt",
        "task_class",
        "obligations",
        "coverage_role",
    ] {
        assert!(
            serialized.get(forbidden).is_none(),
            "unexpected {forbidden}"
        );
    }
}

#[test]
fn duplicate_source_ties_are_deterministic_under_input_permutation() {
    let admissions = vec![admission(
        0,
        "node:alpha",
        PacketAdmissionOriginV1::ExactTypedSelector,
    )];
    let mut first = source("node:alpha", "src/alpha.rs", 1, 3);
    first.source = "zeta".into();
    let mut second = first.clone();
    second.source = "alpha".into();
    let baseline = input(
        admissions.clone(),
        vec![first.clone(), second.clone()],
        Vec::new(),
    );
    let permuted = input(admissions, vec![second, first], Vec::new());
    assert_eq!(
        compile_repository_evidence(&baseline),
        compile_repository_evidence(&permuted)
    );
}

#[test]
fn connecting_forest_is_not_starved_by_sixteen_source_witnesses() {
    let admissions = (0..16)
        .map(|index| {
            admission(
                index,
                &format!("node:{index}"),
                PacketAdmissionOriginV1::Retrieval,
            )
        })
        .collect::<Vec<_>>();
    let sources = (0..16)
        .map(|index| source(&format!("node:{index}"), &format!("src/{index}.rs"), 1, 3))
        .collect::<Vec<_>>();
    let relations = (1..16)
        .map(|index| {
            relation(
                &format!("edge-{index}"),
                &format!("node:{}", index - 1),
                &format!("node:{index}"),
                PacketRelationCertaintyV1::Certain,
            )
        })
        .collect::<Vec<_>>();

    let product = compile_repository_evidence(&input(admissions, sources, relations));
    assert!(product.support.len() <= 16);
    assert!(
        product
            .support
            .iter()
            .any(|unit| unit.kind == SupportUnitKindDto::TypedGraphEdge),
        "a full source set must not erase the admitted-seed connecting forest"
    );
}

#[test]
fn containment_deduplication_preserves_exact_and_retrieved_identities() {
    let product = compile_repository_evidence(&input(
        vec![
            admission(0, "node:exact", PacketAdmissionOriginV1::ExactTypedSelector),
            admission(1, "node:retrieved", PacketAdmissionOriginV1::Retrieval),
        ],
        vec![
            source("node:retrieved", "src/shared.rs", 1, 10),
            source("node:exact", "src/shared.rs", 3, 4),
        ],
        Vec::new(),
    ));

    assert_eq!(
        product
            .support
            .iter()
            .filter(|unit| {
                unit.kind == SupportUnitKindDto::SourceRange
                    && unit.path.as_deref() == Some("src/shared.rs")
            })
            .count(),
        1,
        "contained ranges from one path must not both be emitted"
    );
    assert_eq!(
        product.support[0].symbol_id.as_deref(),
        Some("node:exact"),
        "the exact selector keeps source-bearing precedence"
    );
    assert!(
        product
            .support
            .iter()
            .any(|unit| unit.symbol_id.as_deref() == Some("node:retrieved")),
        "deduplication must not erase the other admitted identity"
    );
}

#[test]
fn distinct_retrieval_paths_precede_repeated_paths_after_exact_sources() {
    let product = compile_repository_evidence(&input(
        vec![
            admission(0, "node:exact", PacketAdmissionOriginV1::ExactTypedSelector),
            admission(1, "node:repeat", PacketAdmissionOriginV1::Retrieval),
            admission(2, "node:distinct", PacketAdmissionOriginV1::Retrieval),
        ],
        vec![
            source("node:exact", "src/shared.rs", 1, 2),
            source("node:repeat", "src/shared.rs", 10, 11),
            source("node:distinct", "src/distinct.rs", 1, 2),
        ],
        Vec::new(),
    ));
    assert_eq!(
        product
            .support
            .iter()
            .filter(|unit| unit.kind == SupportUnitKindDto::SourceRange)
            .filter_map(|unit| unit.symbol_id.as_deref())
            .collect::<Vec<_>>(),
        vec!["node:exact", "node:distinct", "node:repeat"]
    );
}

#[test]
fn uncertain_relations_never_become_compiled_evidence() {
    let product = compile_repository_evidence(&input(
        vec![
            admission(0, "node:left", PacketAdmissionOriginV1::Retrieval),
            admission(1, "node:right", PacketAdmissionOriginV1::Retrieval),
        ],
        vec![
            source("node:left", "src/left.rs", 1, 2),
            source("node:right", "src/right.rs", 1, 2),
        ],
        vec![relation(
            "uncertain",
            "node:left",
            "node:right",
            PacketRelationCertaintyV1::Uncertain,
        )],
    ));
    assert!(
        product
            .support
            .iter()
            .all(|unit| unit.id != "edge:uncertain")
    );
}

#[test]
fn unknown_relation_kinds_never_become_compiled_evidence() {
    let mut unknown = relation(
        "unknown",
        "node:left",
        "node:right",
        PacketRelationCertaintyV1::Certain,
    );
    unknown.relation_kind = PacketRelationKindV1::Unknown;
    let product = compile_repository_evidence(&input(
        vec![
            admission(0, "node:left", PacketAdmissionOriginV1::Retrieval),
            admission(1, "node:right", PacketAdmissionOriginV1::Retrieval),
        ],
        vec![
            source("node:left", "src/left.rs", 1, 2),
            source("node:right", "src/right.rs", 1, 2),
        ],
        vec![unknown],
    ));
    assert!(product.support.iter().all(|unit| unit.id != "edge:unknown"));
}

#[test]
fn admitted_identity_without_source_or_certain_relation_gets_typed_continuation() {
    let product = compile_repository_evidence(&input(
        vec![
            admission(
                0,
                "node:grounded",
                PacketAdmissionOriginV1::ExactTypedSelector,
            ),
            admission(1, "node:orphan", PacketAdmissionOriginV1::Retrieval),
        ],
        vec![source("node:grounded", "src/grounded.rs", 1, 2)],
        Vec::new(),
    ));
    assert!(product.continuation.iter().any(|selector| {
        selector.stable_identity == "node:orphan"
            && selector.reason == PacketStructuralGapReasonV1::DisconnectedSeed
    }));
}

#[test]
fn source_that_cannot_fit_reports_a_typed_source_budget_gap() {
    let mut oversized = source("node:large", "src/large.rs", 1, 2);
    oversized.source = "x".repeat(PUBLIC_PACKET_SERIALIZED_MAX_BYTES * 2);
    let product = compile_repository_evidence(&input(
        vec![admission(
            0,
            "node:large",
            PacketAdmissionOriginV1::ExactTypedSelector,
        )],
        vec![oversized],
        Vec::new(),
    ));
    assert!(
        serde_json::to_vec(&product.support).unwrap().len() <= PUBLIC_PACKET_SERIALIZED_MAX_BYTES
    );
    assert!(product.continuation.iter().any(|selector| {
        selector.stable_identity == "node:large"
            && selector.reason == PacketStructuralGapReasonV1::SourceBudgetExceeded
    }));
}

#[test]
fn bijective_identity_and_path_rename_preserves_compilation_shape() {
    let compile_shape = |prefix: &str| {
        let left = format!("node:{prefix}-left");
        let right = format!("node:{prefix}-right");
        compile_repository_evidence(&input(
            vec![
                admission(0, &left, PacketAdmissionOriginV1::ExactTypedSelector),
                admission(1, &right, PacketAdmissionOriginV1::Retrieval),
            ],
            vec![
                source(&left, &format!("src/{prefix}_left.rs"), 1, 2),
                source(&right, &format!("src/{prefix}_right.rs"), 1, 2),
            ],
            vec![relation(
                "edge",
                &left,
                &right,
                PacketRelationCertaintyV1::Certain,
            )],
        ))
        .support
        .into_iter()
        .map(|unit| unit.kind)
        .collect::<Vec<_>>()
    };
    assert_eq!(compile_shape("alpha"), compile_shape("omega"));
}
