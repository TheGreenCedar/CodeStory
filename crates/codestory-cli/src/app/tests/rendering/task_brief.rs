use super::super::test_support::sample_task_brief_packet;
use crate::app::resolution::quote_command_value;
use crate::app::{build_task_brief_output, render_task_brief_markdown};
use std::path::Path;

#[test]
fn task_brief_output_contract_maps_packet_evidence_to_owner_workflow() {
    let packet = sample_task_brief_packet();
    let brief = build_task_brief_output(Path::new("C:/repo"), &packet);

    assert_eq!(brief.task_brief_version, 1);
    assert_eq!(brief.status, "needs_attention");
    assert_eq!(brief.source_packet_id, "packet-task-brief");
    assert_eq!(brief.source_packet_sufficiency, "partial");
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
    for expected in [
        "codestory-cli packet",
        "codestory-cli snippet",
        "codestory-cli trail",
        "codestory-cli affected",
    ] {
        assert!(
            brief
                .follow_up_codestory_commands
                .iter()
                .any(|command| command.contains(expected)),
            "brief should include {expected}: {brief:#?}"
        );
    }
    assert_eq!(brief.future_sections, ["scout", "where", "onboard"]);

    let packet_command = brief
        .follow_up_codestory_commands
        .iter()
        .find(|command| command.contains("codestory-cli packet"))
        .expect("packet follow-up command");
    assert!(
        packet_command.contains(&format!(
            "--question {}",
            quote_command_value(&packet.question)
        )),
        "packet follow-up should quote prompt safely: {packet_command}"
    );
    let snippet_command = brief
        .follow_up_codestory_commands
        .iter()
        .find(|command| command.contains("codestory-cli snippet"))
        .expect("snippet follow-up command");
    assert!(
        snippet_command.contains(&quote_command_value(&brief.first_files[0].path)),
        "snippet follow-up should quote path safely: {snippet_command}"
    );
    let trail_command = brief
        .follow_up_codestory_commands
        .iter()
        .find(|command| command.contains("codestory-cli trail"))
        .expect("trail follow-up command");
    assert!(
        trail_command.contains(&quote_command_value(&brief.relevant_symbols[0].name)),
        "trail follow-up should quote symbol safely: {trail_command}"
    );

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
        "follow_up_codestory_commands",
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
    assert!(
        markdown.contains("- command:\n    codestory-cli packet"),
        "brief markdown should render commands as indented code blocks: {markdown}"
    );
    assert!(
        !markdown.contains("- `codestory-cli"),
        "brief markdown should not render follow-up commands as inline code: {markdown}"
    );
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
        "## Follow Up CodeStory Commands",
        "## Future Sections",
    ] {
        assert!(
            markdown.contains(heading),
            "brief markdown should include {heading}: {markdown}"
        );
    }
}
