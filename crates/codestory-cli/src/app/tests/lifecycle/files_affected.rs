use crate::app::source_commands::{
    UnsupportedNonUtf8Path, parse_git_name_status_records_z, render_files_markdown,
    unsupported_non_utf8_path_envelope,
};
use codestory_contracts::api::{
    AffectedChangeKindDto, IndexedFileDto, IndexedFileIncompleteReasonCountDto, IndexedFileRoleDto,
    IndexedFilesDto, IndexedFilesSummaryDto, SourcePolicyExclusionDto,
};

#[test]
fn files_markdown_reports_incomplete_reason_text() {
    let output = IndexedFilesDto {
        project_root: "C:/repo".to_string(),
        usable: true,
        summary: IndexedFilesSummaryDto {
            file_count: 1,
            indexed_file_count: 1,
            filtered_file_count: 1,
            visible_file_count: 1,
            incomplete_file_count: 1,
            error_file_count: 0,
            policy_exclusion_count: 0,
            incomplete_reason_counts: vec![IndexedFileIncompleteReasonCountDto {
                reason: "unknown".to_string(),
                file_count: 1,
                detail: "incomplete with no recorded file-level error; run a full reindex"
                    .to_string(),
            }],
            truncated: false,
            language_counts: Vec::new(),
            framework_route_coverage: Vec::new(),
            coverage_notes: Vec::new(),
        },
        coverage_gaps: Vec::new(),
        policy_exclusions: Vec::new(),
        files: vec![IndexedFileDto {
            path: "src/lib.rs".to_string(),
            language: "rust".to_string(),
            indexed: true,
            complete: false,
            line_count: 1,
            role: IndexedFileRoleDto::Source,
            error_count: 0,
        }],
    };

    let markdown = render_files_markdown(&output);

    assert!(
        markdown.contains("- incomplete_reasons: unknown=1"),
        "{markdown}"
    );
    assert!(
        markdown.contains("run a full reindex"),
        "incomplete counts need operator-actionable reason text: {markdown}"
    );
}

#[test]
fn files_markdown_labels_verified_policy_exclusions_as_non_graph_evidence() {
    let output = IndexedFilesDto {
            project_root: "/repo".into(),
            usable: true,
            summary: IndexedFilesSummaryDto {
                file_count: 1,
                indexed_file_count: 1,
                filtered_file_count: 1,
                visible_file_count: 1,
                incomplete_file_count: 0,
                error_file_count: 0,
                policy_exclusion_count: 1,
                incomplete_reason_counts: Vec::new(),
                truncated: false,
                language_counts: Vec::new(),
                framework_route_coverage: Vec::new(),
                coverage_notes: vec![
                    "1 verified source policy exclusion has no parser-backed graph or semantic coverage"
                        .into(),
                ],
            },
            coverage_gaps: Vec::new(),
            policy_exclusions: vec![SourcePolicyExclusionDto {
                path: "vendor/registers.h".into(),
                role: IndexedFileRoleDto::Vendor,
                content_hash: "a".repeat(64),
                observed_size: 279_751,
                observed_unit_count: 4_514,
                policy_version: "bounded-source-exclusion-v2".into(),
                byte_cap: 1_000_000,
                structural_unit_cap:
                    codestory_contracts::workspace::DEFAULT_STRUCTURAL_UNIT_CAP,
                project_id: "project".into(),
                workspace_id: "workspace".into(),
                core_generation_id: "generation".into(),
                core_run_id: "run".into(),
                graph_coverage: false,
                semantic_coverage: false,
            }],
            files: Vec::new(),
        };

    let markdown = render_files_markdown(&output);
    assert!(markdown.contains("policy exclusions: 1"), "{markdown}");
    assert!(
        markdown.contains("source inventory only; no graph or semantic coverage"),
        "{markdown}"
    );
    assert!(markdown.contains("vendor/registers.h"), "{markdown}");
    assert!(markdown.contains("4514 structural units"), "{markdown}");
    assert!(markdown.contains("unit_cap=2048"), "{markdown}");
}

#[test]
fn affected_name_status_parser_preserves_nul_delimited_special_paths() {
    let records = parse_git_name_status_records_z(
            b"M\0 leading and trailing \t\n \0D\0src/old.ts\0R100\0 before.ts \0after\nname.ts\0C75\0src/base.ts\0src/copy.ts\0",
        )
        .expect("parse NUL-delimited name-status");

    assert_eq!(records[0].kind, AffectedChangeKindDto::Modified);
    assert_eq!(records[0].status, "M");
    assert_eq!(records[0].path, " leading and trailing \t\n ");
    assert_eq!(records[1].kind, AffectedChangeKindDto::Deleted);
    assert_eq!(records[2].kind, AffectedChangeKindDto::Renamed);
    assert_eq!(records[2].previous_path.as_deref(), Some(" before.ts "));
    assert_eq!(records[2].path, "after\nname.ts");
    assert_eq!(records[3].kind, AffectedChangeKindDto::Copied);
    assert_eq!(records[3].previous_path.as_deref(), Some("src/base.ts"));
}

#[test]
fn affected_non_utf8_git_path_has_a_typed_failure_envelope() {
    let error = parse_git_name_status_records_z(b"M\0src/invalid-\xff.rs\0")
        .expect_err("non-UTF-8 Git paths cannot enter string DTOs");
    let unsupported = error
        .downcast_ref::<UnsupportedNonUtf8Path>()
        .expect("typed non-UTF-8 path error");
    let envelope = unsupported_non_utf8_path_envelope(unsupported);

    assert_eq!(envelope.error.code, "unsupported_non_utf8_path");
    assert_eq!(
        envelope
            .error
            .details
            .as_deref()
            .and_then(|details| details.failed_layer.as_deref()),
        Some("git_change_discovery")
    );
    assert!(!unsupported.to_string().contains('\u{fffd}'));
}
