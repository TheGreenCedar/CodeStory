use super::super::test_support::sample_task_brief_packet;
use crate::app::{build_task_brief_output, render_task_brief_markdown};
use codestory_contracts::api::{
    BoundedDrillPlanDto, DrillGapKindDto, DrillOptionDto, PacketDispositionDto,
};

#[test]
fn task_brief_output_contract_maps_packet_evidence_to_owner_workflow() {
    let packet = sample_task_brief_packet();
    let brief = build_task_brief_output(&packet);

    assert_eq!(brief.task_brief_version, 2);
    assert_eq!(brief.status, "ready");
    assert_eq!(brief.source_packet_id, "packet-task-brief");
    assert_eq!(brief.source_packet_disposition, "supported");
    assert_eq!(
        brief.first_files[0].path,
        "crates/codestory-cli/src/`main_$env:SECRET$('x').rs"
    );
    assert_eq!(
        brief.relevant_symbols[0].name,
        "run_`packet_$env:SECRET$('x')"
    );
    assert_eq!(
        brief.likely_tests[0].path,
        "crates/codestory-cli/tests/stdio`$env:SECRET$('x')_protocol_contracts.rs"
    );
    assert!(
        brief
            .impacted_surfaces
            .contains(&"crates/codestory-cli".to_string())
    );
    assert!(
        brief
            .risks_unknowns
            .contains(&"verify `changed` files after editing".to_string())
    );
    assert_eq!(brief.packet_continuation, None);
    assert_eq!(brief.future_sections, ["scout", "where", "onboard"]);

    let json = serde_json::to_value(&brief).expect("brief should serialize");
    for key in [
        "task_brief_version",
        "prompt",
        "status",
        "first_files",
        "relevant_symbols",
        "likely_tests",
        "impacted_surfaces",
        "risks_unknowns",
        "source_packet_disposition",
        "packet_continuation",
        "future_sections",
    ] {
        assert!(json.get(key).is_some(), "brief JSON should include {key}");
    }

    let markdown = render_task_brief_markdown(&brief);
    assert!(
        markdown.contains("prompt: `Add '$env:SECRET $(Get-ChildItem) 'literal' task brief`"),
        "brief markdown should replace prompt backticks inside inline code: {markdown}"
    );
    assert!(
        markdown.contains("`crates/codestory-cli/src/'main_$env:SECRET$('x').rs`"),
        "brief markdown should replace path backticks inside inline code: {markdown}"
    );
    assert!(
        markdown.contains("`run_'packet_$env:SECRET$('x')`"),
        "brief markdown should replace symbol backticks inside inline code: {markdown}"
    );
    assert!(
        markdown.contains("- verify 'changed' files after editing"),
        "brief markdown should replace risk backticks in bullets: {markdown}"
    );
    assert!(markdown.contains("- none; source packet disposition is terminal"));
    assert!(!markdown.contains("codestory-cli snippet"));
    assert!(!markdown.contains("codestory-cli trail"));
    assert!(
        !markdown.contains("```"),
        "brief markdown should not use fences that embedded backticks can split: {markdown}"
    );
    for heading in [
        "# Task Brief",
        "## First Files",
        "## Relevant Symbols",
        "## Likely Tests",
        "## Impacted Surfaces",
        "## Risks And Unknowns",
        "## Packet Continuation",
        "## Future Sections",
    ] {
        assert!(
            markdown.contains(heading),
            "brief markdown should include {heading}: {markdown}"
        );
    }
}

#[test]
fn task_brief_exposes_only_the_typed_drill_once_continuation() {
    let mut packet = sample_task_brief_packet();
    packet.disposition = PacketDispositionDto::drill_once(
        "one bounded source gap remains",
        BoundedDrillPlanDto {
            parent_packet_id: packet.packet_id.clone(),
            core_generation_id: "core-generation-7".to_string(),
            retrieval_generation: Some("retrieval-generation-4".to_string()),
            gap_ids: vec!["gap-1".to_string()],
            options: vec![DrillOptionDto {
                id: "option-1".to_string(),
                gap_id: "gap-1".to_string(),
                kind: DrillGapKindDto::BoundedSourceRead,
                path: Some("src/lib.rs".to_string()),
                symbol_id: None,
                structural_reason: Some(
                    codestory_contracts::compilation::PacketStructuralGapReasonV1::SourceUnavailable,
                ),
            }],
            max_bytes: 32 * 1024,
            max_hits: 8,
            max_depth: 2,
            remaining_rounds: 1,
        },
    );

    let brief = build_task_brief_output(&packet);
    let continuation = brief
        .packet_continuation
        .as_ref()
        .expect("drill_once task brief should retain the typed continuation");
    assert_eq!(continuation.parent_packet_id, packet.packet_id);
    assert_eq!(continuation.core_generation_id, "core-generation-7");
    assert_eq!(
        continuation.retrieval_generation.as_deref(),
        Some("retrieval-generation-4")
    );
    assert_eq!(continuation.remaining_rounds, 1);
    assert_eq!(continuation.options[0].id, "option-1");

    let markdown = render_task_brief_markdown(&brief);
    assert!(markdown.contains("parent_packet_id: `packet-task-brief`"));
    assert!(markdown.contains("option_ids: `option-1`"));
    assert!(!markdown.contains("codestory-cli packet"));
    assert!(!markdown.contains("codestory-cli snippet"));
    assert!(!markdown.contains("codestory-cli trail"));
}
