use super::super::artifacts::{ensure_dot_only_for_trail, preflight_output_file};
use super::super::lifecycle::{OpenedAgentSurface, open_agent_surface};
use super::super::resolution::{quote_command_path, quote_command_value};
use super::packet::{
    packet_budget_mode_label, packet_budget_omitted_sections, packet_operator_status,
    packet_sufficiency_label,
};
use crate::args;
use crate::args::{TaskAction, TaskBriefCommand, TaskCommand};
use crate::display;
use crate::output::{RenderedPublicOutput, emit_public_operation};
use crate::runtime::map_api_error;
use anyhow::Result;
use codestory_contracts::api::{AgentPacketDto, AgentPacketRequestDto, PacketTaskClassDto};
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

    let operation = runtime.run_public_operation("packet", || {
        let packet = runtime
            .browser
            .packet(AgentPacketRequestDto {
                question: cmd.prompt.clone(),
                budget: cmd.budget.into(),
                task_class: Some(PacketTaskClassDto::EditPlanning),
                probes: cmd.probes.clone(),
                extra_probes: cmd.extra_probes.clone(),
                include_evidence: !cmd.no_evidence,
                latency_budget_ms: cmd.latency_budget_ms,
            })
            .map_err(map_api_error)?;
        let brief = build_task_brief_output(&runtime.project_root, &packet);
        let markdown = render_task_brief_markdown(&brief);
        RenderedPublicOutput::structured(&brief, markdown)
    })?;
    emit_public_operation(cmd.format, operation, cmd.output_file.as_deref())
}

#[derive(Debug, serde::Serialize)]
struct TaskBriefOutput {
    task_brief_version: u32,
    prompt: String,
    status: String,
    source_packet_id: String,
    source_packet_sufficiency: String,
    first_files: Vec<TaskBriefFileOutput>,
    relevant_symbols: Vec<TaskBriefSymbolOutput>,
    likely_tests: Vec<TaskBriefFileOutput>,
    impacted_surfaces: Vec<String>,
    risks_unknowns: Vec<String>,
    follow_up_codestory_commands: Vec<String>,
    future_sections: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
struct TaskBriefFileOutput {
    path: String,
    line: Option<u32>,
    reason: String,
}

#[derive(Debug, Clone, serde::Serialize)]
struct TaskBriefSymbolOutput {
    name: String,
    kind: String,
    path: Option<String>,
    line: Option<u32>,
    reason: String,
}

fn build_task_brief_output(
    project_root: &std::path::Path,
    packet: &AgentPacketDto,
) -> TaskBriefOutput {
    let citations = packet_task_brief_citations(packet);
    let first_files = task_brief_first_files(&citations);
    let relevant_symbols = task_brief_relevant_symbols(&citations);
    let likely_tests = task_brief_likely_tests(&citations);
    let impacted_surfaces = task_brief_impacted_surfaces(&first_files, &relevant_symbols);
    let risks_unknowns = task_brief_risks_unknowns(packet, &likely_tests);
    let follow_up_codestory_commands =
        task_brief_follow_up_commands(project_root, packet, &first_files, &relevant_symbols);

    TaskBriefOutput {
        task_brief_version: 1,
        prompt: packet.question.clone(),
        status: packet_operator_status(packet.sufficiency.status).to_string(),
        source_packet_id: packet.packet_id.clone(),
        source_packet_sufficiency: packet_sufficiency_label(packet.sufficiency.status).to_string(),
        first_files,
        relevant_symbols,
        likely_tests,
        impacted_surfaces,
        risks_unknowns,
        follow_up_codestory_commands,
        future_sections: vec![
            "scout".to_string(),
            "where".to_string(),
            "onboard".to_string(),
        ],
    }
}

fn packet_task_brief_citations(
    packet: &AgentPacketDto,
) -> Vec<&codestory_contracts::api::AgentCitationDto> {
    let mut citations = Vec::new();
    for claim in &packet.sufficiency.covered_claims {
        citations.extend(claim.citations.iter());
    }
    citations.extend(packet.answer.citations.iter());
    citations
}

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

fn task_brief_risks_unknowns(
    packet: &AgentPacketDto,
    likely_tests: &[TaskBriefFileOutput],
) -> Vec<String> {
    let mut risks = packet.sufficiency.gaps.clone();
    if packet.budget.truncated {
        risks.push(format!(
            "source packet was budget-truncated; omitted sections: {}",
            packet_budget_omitted_sections(packet)
        ));
    }
    if likely_tests.is_empty() {
        risks.push("no test files were cited by the source packet".to_string());
    }
    if risks.is_empty() {
        risks.push("none from packet sufficiency; verify cited files before editing".to_string());
    }
    risks
}

fn task_brief_follow_up_commands(
    project_root: &std::path::Path,
    packet: &AgentPacketDto,
    first_files: &[TaskBriefFileOutput],
    symbols: &[TaskBriefSymbolOutput],
) -> Vec<String> {
    let project = quote_command_path(project_root);
    let prompt = quote_command_value(&packet.question);
    let mut commands = Vec::new();
    commands.push(format!(
        "codestory-cli packet --project {project} --question {prompt} --task-class edit-planning --budget {}",
        packet_budget_mode_label(packet.budget.requested)
    ));
    if let Some(file) = first_files.first() {
        commands.push(format!(
            "codestory-cli snippet --project {project} --query {}",
            quote_command_value(&file.path)
        ));
    }
    if let Some(symbol) = symbols.first() {
        commands.push(format!(
            "codestory-cli trail --project {project} --query {} --story --hide-speculative",
            quote_command_value(&symbol.name)
        ));
    }
    commands.push(format!("codestory-cli affected --project {project} <path>"));
    commands.extend(packet.sufficiency.follow_up_commands.iter().cloned());
    commands
}

fn render_task_brief_markdown(brief: &TaskBriefOutput) -> String {
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
        "source_packet_sufficiency: {}",
        task_brief_inline_code(&brief.source_packet_sufficiency)
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
    append_task_brief_commands(
        &mut markdown,
        "Follow Up CodeStory Commands",
        &brief.follow_up_codestory_commands,
    );
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

fn append_task_brief_commands(markdown: &mut String, title: &str, values: &[String]) {
    let _ = writeln!(markdown, "\n## {title}");
    for value in values {
        let _ = writeln!(markdown, "- command:");
        let _ = writeln!(markdown, "    {}", value.replace(['\r', '\n'], " "));
    }
}
