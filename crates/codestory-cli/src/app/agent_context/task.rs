use super::super::artifacts::{ensure_dot_only_for_trail, preflight_output_file};
use super::super::lifecycle::{OpenedAgentSurface, open_agent_surface};
#[cfg(test)]
use super::packet::{
    packet_budget_omitted_sections, packet_disposition_label, packet_operator_status,
};
use crate::args;
use crate::args::{TaskAction, TaskBriefCommand, TaskCommand};
#[cfg(test)]
use crate::display;
use crate::output::{RenderedPublicOutput, emit_public_operation};
use crate::runtime::map_api_error;
use anyhow::Result;
use codestory_contracts::api::AgentPacketRequestDto;
#[cfg(test)]
use codestory_contracts::api::{AgentPacketDto, BoundedDrillPlanDto, PacketDispositionKindDto};
use codestory_contracts::packet_projection_v3::{
    ContinuationStateV3Dto, EvidenceAvailabilityV3Dto, PacketProjectionV3Dto,
};
#[cfg(test)]
use std::collections::BTreeSet;
use std::fmt::Write as _;

pub(in crate::app) fn run_task(cmd: TaskCommand) -> Result<()> {
    match cmd.action {
        TaskAction::Brief(cmd) => run_task_brief(cmd),
    }
}

fn run_task_brief(cmd: TaskBriefCommand) -> Result<()> {
    ensure_dot_only_for_trail(cmd.format, "task brief")?;
    preflight_output_file(cmd.output_file.as_deref())?;
    args::validate_packet_probe_arguments(&cmd.probes, &cmd.extra_probes)
        .map_err(anyhow::Error::msg)?;
    let OpenedAgentSurface { runtime, .. } =
        open_agent_surface(&cmd.project, None, None, cmd.refresh, "task brief")?;

    let request = AgentPacketRequestDto {
        question: cmd.prompt.clone(),
        budget: cmd.budget.into(),
        probes: cmd.probes.clone(),
        extra_probes: cmd.extra_probes.clone(),
        latency_budget_ms: cmd.latency_budget_ms,
        parent_packet_id: None,
        option_ids: Vec::new(),
        core_generation_id: None,
        retrieval_generation: None,
    };
    let operation = runtime.run_public_operation("packet", || {
        let packet = runtime
            .browser
            .packet(request.clone())
            .map_err(map_api_error)?;
        let product = codestory_runtime::project_packet_v3(
            &runtime.public_operation,
            "codestory-cli-task-brief",
            &request,
            &packet,
            |candidate| {
                serde_json::to_vec(candidate)
                    .map(|bytes| bytes.len())
                    .map_err(|_| ())
            },
        )
        .map_err(map_api_error)?;
        let brief = build_task_brief_v3_output(&cmd.prompt, &product.projection);
        let markdown = render_task_brief_v3_markdown(&brief);
        RenderedPublicOutput::structured(&brief, markdown)
    })?;
    emit_public_operation(cmd.format, operation, cmd.output_file.as_deref())
}

#[derive(Debug, serde::Serialize)]
struct TaskBriefV3Output {
    task_brief_version: u32,
    prompt: String,
    evidence_availability: String,
    source_packet_id: String,
    first_files: Vec<TaskBriefFileOutput>,
    relevant_symbols: Vec<TaskBriefSymbolOutput>,
    risks_unknowns: Vec<String>,
    packet_continuation: Option<ContinuationStateV3Dto>,
}

fn build_task_brief_v3_output(
    prompt: &str,
    projection: &PacketProjectionV3Dto,
) -> TaskBriefV3Output {
    match projection {
        PacketProjectionV3Dto::Complete {
            identity,
            status,
            evidence,
            gaps,
            continuation,
            ..
        } => {
            let first_files = evidence
                .as_slice()
                .iter()
                .filter_map(|row| {
                    row.path.as_ref().map(|path| TaskBriefFileOutput {
                        path: path.as_str().to_owned(),
                        line: row.start_line,
                        reason: row
                            .summary
                            .as_ref()
                            .map_or("packet evidence".to_string(), |summary| {
                                summary.as_str().to_owned()
                            }),
                    })
                })
                .take(8)
                .collect();
            let relevant_symbols = evidence
                .as_slice()
                .iter()
                .filter_map(|row| {
                    row.symbol_id.as_ref().map(|symbol| TaskBriefSymbolOutput {
                        name: symbol.as_str().to_owned(),
                        kind: "indexed_symbol".to_string(),
                        path: row.path.as_ref().map(|path| path.as_str().to_owned()),
                        line: row.start_line,
                        reason: row
                            .summary
                            .as_ref()
                            .map_or("packet evidence".to_string(), |summary| {
                                summary.as_str().to_owned()
                            }),
                    })
                })
                .take(12)
                .collect();
            let mut risks_unknowns = gaps
                .as_slice()
                .iter()
                .map(|gap| {
                    gap.message.as_ref().map_or_else(
                        || format!("evidence gap {}", gap.identity.gap_id.as_str()),
                        |message| message.as_str().to_owned(),
                    )
                })
                .collect::<Vec<_>>();
            if risks_unknowns.is_empty() {
                risks_unknowns.push("verify affected files and tests before editing".to_string());
            }
            TaskBriefV3Output {
                task_brief_version: 3,
                prompt: prompt.to_owned(),
                evidence_availability: evidence_availability_label(status).to_string(),
                source_packet_id: identity.packet_id.as_str().to_owned(),
                first_files,
                relevant_symbols,
                risks_unknowns,
                packet_continuation: continuation.clone(),
            }
        }
        PacketProjectionV3Dto::BudgetExceeded { identity, gaps, .. } => {
            debug_assert_eq!(gaps.as_slice().len(), 1);
            debug_assert_eq!(
                gaps.as_slice()[0].kind,
                codestory_contracts::packet_projection_v3::GapKindV3Dto::OutputBudgetExceeded
            );
            TaskBriefV3Output {
                task_brief_version: 3,
                prompt: prompt.to_owned(),
                evidence_availability: "unavailable".to_string(),
                source_packet_id: identity.packet_id.as_str().to_owned(),
                first_files: Vec::new(),
                relevant_symbols: Vec::new(),
                risks_unknowns: vec![
                    "output_budget_exceeded: packet projection exceeded the public result budget"
                        .to_string(),
                ],
                packet_continuation: None,
            }
        }
    }
}

fn evidence_availability_label(status: &EvidenceAvailabilityV3Dto) -> &'static str {
    match status {
        EvidenceAvailabilityV3Dto::Available => "available",
        EvidenceAvailabilityV3Dto::ContinuationAvailable => "continuation_available",
        EvidenceAvailabilityV3Dto::NoUsefulEvidence => "no_useful_evidence",
        EvidenceAvailabilityV3Dto::Unavailable => "unavailable",
    }
}

fn render_task_brief_v3_markdown(brief: &TaskBriefV3Output) -> String {
    let mut markdown = String::from("# Task Brief\n");
    let _ = writeln!(markdown, "task_brief_version: {}", brief.task_brief_version);
    let _ = writeln!(
        markdown,
        "evidence_availability: `{}`",
        brief.evidence_availability
    );
    let _ = writeln!(markdown, "source_packet_id: `{}`", brief.source_packet_id);
    let _ = writeln!(
        markdown,
        "prompt: `{}`",
        task_brief_markdown_text(&brief.prompt)
    );
    append_task_brief_files(&mut markdown, "First Files", &brief.first_files);
    append_task_brief_symbols(&mut markdown, "Relevant Symbols", &brief.relevant_symbols);
    append_task_brief_strings(&mut markdown, "Risks And Unknowns", &brief.risks_unknowns);
    let _ = writeln!(markdown, "\n## Packet Continuation");
    if let Some(continuation) = &brief.packet_continuation {
        let _ = writeln!(
            markdown,
            "- continuation_id: `{}`",
            continuation.continuation_id.as_str()
        );
        let _ = writeln!(
            markdown,
            "- remaining_rounds: {}",
            continuation.remaining_rounds
        );
    } else {
        let _ = writeln!(markdown, "- none");
    }
    markdown
}

#[cfg(test)]
#[derive(Debug, serde::Serialize)]
pub(in crate::app) struct TaskBriefOutput {
    pub(in crate::app) task_brief_version: u32,
    pub(in crate::app) prompt: String,
    pub(in crate::app) status: String,
    pub(in crate::app) source_packet_id: String,
    pub(in crate::app) source_packet_disposition: String,
    pub(in crate::app) first_files: Vec<TaskBriefFileOutput>,
    pub(in crate::app) relevant_symbols: Vec<TaskBriefSymbolOutput>,
    pub(in crate::app) likely_tests: Vec<TaskBriefFileOutput>,
    pub(in crate::app) impacted_surfaces: Vec<String>,
    pub(in crate::app) risks_unknowns: Vec<String>,
    pub(in crate::app) packet_continuation: Option<BoundedDrillPlanDto>,
    pub(in crate::app) future_sections: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub(in crate::app) struct TaskBriefFileOutput {
    pub(in crate::app) path: String,
    pub(in crate::app) line: Option<u32>,
    pub(in crate::app) reason: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub(in crate::app) struct TaskBriefSymbolOutput {
    pub(in crate::app) name: String,
    pub(in crate::app) kind: String,
    pub(in crate::app) path: Option<String>,
    pub(in crate::app) line: Option<u32>,
    pub(in crate::app) reason: String,
}

#[cfg(test)]
pub(in crate::app) fn build_task_brief_output(packet: &AgentPacketDto) -> TaskBriefOutput {
    let citations = packet_task_brief_citations(packet);
    let first_files = task_brief_first_files(&citations);
    let relevant_symbols = task_brief_relevant_symbols(&citations);
    let likely_tests = task_brief_likely_tests(&citations);
    let impacted_surfaces = task_brief_impacted_surfaces(&first_files, &relevant_symbols);
    let risks_unknowns = task_brief_risks_unknowns(packet, &likely_tests);

    TaskBriefOutput {
        task_brief_version: 2,
        prompt: packet.question.clone(),
        status: packet_operator_status(packet.disposition.kind).to_string(),
        source_packet_id: packet.packet_id.clone(),
        source_packet_disposition: packet_disposition_label(packet.disposition.kind).to_string(),
        first_files,
        relevant_symbols,
        likely_tests,
        impacted_surfaces,
        risks_unknowns,
        packet_continuation: packet.disposition.drill.clone(),
        future_sections: vec![
            "scout".to_string(),
            "where".to_string(),
            "onboard".to_string(),
        ],
    }
}

#[cfg(test)]
fn packet_task_brief_citations(
    packet: &AgentPacketDto,
) -> Vec<&codestory_contracts::api::AgentCitationDto> {
    let mut citations = Vec::new();
    citations.extend(packet.answer.citations.iter());
    citations
}

#[cfg(test)]
fn task_brief_first_files(
    citations: &[&codestory_contracts::api::AgentCitationDto],
) -> Vec<TaskBriefFileOutput> {
    let mut seen = BTreeSet::new();
    let mut files = Vec::new();
    for citation in citations {
        let Some(path) = citation.file_path.as_deref() else {
            continue;
        };
        if seen.insert(path.to_string()) {
            files.push(TaskBriefFileOutput {
                path: path.to_string(),
                line: citation.line,
                reason: "cited by source packet".to_string(),
            });
        }
        if files.len() >= 8 {
            break;
        }
    }
    files
}

#[cfg(test)]
fn task_brief_relevant_symbols(
    citations: &[&codestory_contracts::api::AgentCitationDto],
) -> Vec<TaskBriefSymbolOutput> {
    let mut seen = BTreeSet::new();
    let mut symbols = Vec::new();
    for citation in citations {
        let key = format!(
            "{}:{}:{}",
            citation.display_name,
            citation.file_path.as_deref().unwrap_or(""),
            citation.line.unwrap_or(0)
        );
        if seen.insert(key) {
            symbols.push(TaskBriefSymbolOutput {
                name: citation.display_name.clone(),
                kind: display::format_kind(citation.kind),
                path: citation.file_path.clone(),
                line: citation.line,
                reason: "cited by source packet".to_string(),
            });
        }
        if symbols.len() >= 12 {
            break;
        }
    }
    symbols
}

#[cfg(test)]
fn task_brief_likely_tests(
    citations: &[&codestory_contracts::api::AgentCitationDto],
) -> Vec<TaskBriefFileOutput> {
    let mut seen = BTreeSet::new();
    let mut tests = Vec::new();
    for citation in citations {
        let Some(path) = citation.file_path.as_deref() else {
            continue;
        };
        if task_brief_path_is_test(path) && seen.insert(path.to_string()) {
            tests.push(TaskBriefFileOutput {
                path: path.to_string(),
                line: citation.line,
                reason: "test-like cited file".to_string(),
            });
        }
        if tests.len() >= 6 {
            break;
        }
    }
    tests
}

#[cfg(test)]
fn task_brief_path_is_test(path: &str) -> bool {
    let normalized = path.replace('\\', "/").to_ascii_lowercase();
    normalized.contains("/tests/")
        || normalized.ends_with("_test.rs")
        || normalized.ends_with("_tests.rs")
        || normalized.ends_with(".test.ts")
        || normalized.ends_with(".spec.ts")
        || normalized.ends_with(".test.js")
        || normalized.ends_with(".spec.js")
}

#[cfg(test)]
fn task_brief_impacted_surfaces(
    first_files: &[TaskBriefFileOutput],
    symbols: &[TaskBriefSymbolOutput],
) -> Vec<String> {
    let mut surfaces = BTreeSet::new();
    for path in first_files
        .iter()
        .map(|file| file.path.as_str())
        .chain(symbols.iter().filter_map(|symbol| symbol.path.as_deref()))
    {
        surfaces.insert(task_brief_surface_for_path(path));
    }
    surfaces.into_iter().take(8).collect()
}

#[cfg(test)]
fn task_brief_surface_for_path(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    let mut parts = normalized.split('/');
    match (parts.next(), parts.next()) {
        (Some("crates"), Some(crate_name)) => format!("crates/{crate_name}"),
        (Some(first), Some(second)) if first == "plugins" => format!("{first}/{second}"),
        (Some(first), _) if !first.is_empty() => first.to_string(),
        _ => "unknown".to_string(),
    }
}

#[cfg(test)]
fn task_brief_risks_unknowns(
    packet: &AgentPacketDto,
    likely_tests: &[TaskBriefFileOutput],
) -> Vec<String> {
    let mut risks = packet.disposition.omission_receipts.clone();
    if packet.budget.truncated {
        risks.push(format!(
            "source packet was budget-truncated; omitted sections: {}",
            packet_budget_omitted_sections(packet)
        ));
    }
    if likely_tests.is_empty() {
        risks.push("no test files were cited by the source packet".to_string());
    }
    if packet.disposition.kind == PacketDispositionKindDto::Supported && risks.is_empty() {
        risks.push("verify `changed` files after editing".to_string());
    }
    if risks.is_empty() {
        risks.push("none from packet disposition; verify cited files before editing".to_string());
    }
    risks
}

#[cfg(test)]
pub(in crate::app) fn render_task_brief_markdown(brief: &TaskBriefOutput) -> String {
    let mut markdown = String::new();
    let _ = writeln!(markdown, "# Task Brief");
    let _ = writeln!(
        markdown,
        "status: {}",
        task_brief_inline_code(&brief.status)
    );
    let _ = writeln!(markdown, "task_brief_version: {}", brief.task_brief_version);
    let _ = writeln!(
        markdown,
        "source_packet_id: {}",
        task_brief_inline_code(&brief.source_packet_id)
    );
    let _ = writeln!(
        markdown,
        "source_packet_disposition: {}",
        task_brief_inline_code(&brief.source_packet_disposition)
    );
    let _ = writeln!(
        markdown,
        "prompt: {}",
        task_brief_inline_code(&brief.prompt)
    );
    append_task_brief_files(&mut markdown, "First Files", &brief.first_files);
    append_task_brief_symbols(&mut markdown, "Relevant Symbols", &brief.relevant_symbols);
    append_task_brief_files(&mut markdown, "Likely Tests", &brief.likely_tests);
    append_task_brief_strings(&mut markdown, "Impacted Surfaces", &brief.impacted_surfaces);
    append_task_brief_strings(&mut markdown, "Risks And Unknowns", &brief.risks_unknowns);
    append_task_brief_continuation(&mut markdown, brief.packet_continuation.as_ref());
    append_task_brief_strings(&mut markdown, "Future Sections", &brief.future_sections);
    markdown
}

fn task_brief_inline_code(value: &str) -> String {
    format!("`{}`", task_brief_markdown_text(value))
}

fn task_brief_markdown_text(value: &str) -> String {
    value.replace('`', "'").replace(['\r', '\n'], " ")
}

fn append_task_brief_files(markdown: &mut String, title: &str, files: &[TaskBriefFileOutput]) {
    let _ = writeln!(markdown, "\n## {title}");
    if files.is_empty() {
        let _ = writeln!(markdown, "- none from source packet");
        return;
    }
    for file in files {
        let line = file.line.map(|line| format!(":{line}")).unwrap_or_default();
        let _ = writeln!(
            markdown,
            "- {}{} - {}",
            task_brief_inline_code(&file.path),
            line,
            task_brief_markdown_text(&file.reason)
        );
    }
}

fn append_task_brief_symbols(
    markdown: &mut String,
    title: &str,
    symbols: &[TaskBriefSymbolOutput],
) {
    let _ = writeln!(markdown, "\n## {title}");
    if symbols.is_empty() {
        let _ = writeln!(markdown, "- none from source packet");
        return;
    }
    for symbol in symbols {
        let location = symbol
            .path
            .as_ref()
            .map(|path| {
                let line = symbol
                    .line
                    .map(|line| format!(":{line}"))
                    .unwrap_or_default();
                format!(" {}{line}", task_brief_inline_code(path))
            })
            .unwrap_or_default();
        let _ = writeln!(
            markdown,
            "- {} ({}){} - {}",
            task_brief_inline_code(&symbol.name),
            task_brief_markdown_text(&symbol.kind),
            location,
            task_brief_markdown_text(&symbol.reason)
        );
    }
}

fn append_task_brief_strings(markdown: &mut String, title: &str, values: &[String]) {
    let _ = writeln!(markdown, "\n## {title}");
    if values.is_empty() {
        let _ = writeln!(markdown, "- none");
        return;
    }
    for value in values {
        let _ = writeln!(markdown, "- {}", task_brief_markdown_text(value));
    }
}

#[cfg(test)]
fn append_task_brief_continuation(
    markdown: &mut String,
    continuation: Option<&BoundedDrillPlanDto>,
) {
    let _ = writeln!(markdown, "\n## Packet Continuation");
    let Some(continuation) = continuation else {
        let _ = writeln!(markdown, "- none; source packet disposition is terminal");
        return;
    };
    let _ = writeln!(
        markdown,
        "- parent_packet_id: {}",
        task_brief_inline_code(&continuation.parent_packet_id)
    );
    let _ = writeln!(
        markdown,
        "- core_generation_id: {}",
        task_brief_inline_code(&continuation.core_generation_id)
    );
    if let Some(retrieval_generation) = continuation.retrieval_generation.as_deref() {
        let _ = writeln!(
            markdown,
            "- retrieval_generation: {}",
            task_brief_inline_code(retrieval_generation)
        );
    }
    let _ = writeln!(
        markdown,
        "- remaining_rounds: {}",
        continuation.remaining_rounds
    );
    let option_ids = continuation
        .options
        .iter()
        .map(|option| option.id.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let _ = writeln!(
        markdown,
        "- option_ids: {}",
        task_brief_inline_code(&option_ids)
    );
}
