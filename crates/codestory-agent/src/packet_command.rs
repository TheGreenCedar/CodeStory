use codestory_contracts::api::{
    BoundedDrillPlanDto, PacketBudgetModeDto, PacketFollowUpInvocationDto,
};
use std::path::Path;

pub fn packet_argv(arguments: &[&str]) -> Vec<String> {
    std::iter::once("codestory-cli".to_string())
        .chain(arguments.iter().map(|argument| (*argument).to_string()))
        .collect()
}

/// Split argv into the published `{program, args}` invocation.
pub fn packet_follow_up_invocation(argv: &[String]) -> PacketFollowUpInvocationDto {
    let (program, args) = argv.split_first().expect("packet argv carries a program");
    PacketFollowUpInvocationDto {
        program: program.clone(),
        args: args.to_vec(),
    }
}

/// Render argv as one copy-pasteable POSIX shell command.
///
/// Arguments made only of characters a shell passes through untouched stay
/// bare so the suggestion reads like something a person would type; everything
/// else is single-quoted.
pub fn render_packet_command(argv: &[String]) -> String {
    argv.iter()
        .map(|argument| {
            if packet_command_argument_is_shell_safe(argument) {
                argument.clone()
            } else {
                quote_packet_command_value(argument)
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn packet_command_argument_is_shell_safe(argument: &str) -> bool {
    !argument.is_empty()
        && argument
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "_@%+=:,./-".contains(character))
}

/// The project argument as the caller would type it, before any quoting.
pub fn packet_display_project_arg(project_root: &Path) -> String {
    project_root.to_string_lossy().into_owned()
}

/// Quote one argv element for a POSIX shell.
///
/// Doubling the apostrophe is the PowerShell/SQL convention; `sh` concatenates
/// the adjacent quoted runs and drops the character, so an argument has to end
/// the quoted run, escape the apostrophe, and reopen it.
pub fn quote_packet_command_value(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub fn packet_budget_cli_name(requested: PacketBudgetModeDto) -> &'static str {
    match requested {
        PacketBudgetModeDto::Tiny => "tiny",
        PacketBudgetModeDto::Compact => "compact",
        PacketBudgetModeDto::Standard => "standard",
        PacketBudgetModeDto::Deep => "deep",
    }
}

pub fn next_deeper_packet_command(
    project_root: &Path,
    question: &str,
    requested: PacketBudgetModeDto,
) -> Option<String> {
    next_deeper_packet_argv(project_root, question, requested)
        .map(|argv| render_packet_command(&argv))
}

/// The deeper-budget retry as executable argv; the displayed command renders
/// from this, never the other way round.
pub fn next_deeper_packet_argv(
    project_root: &Path,
    question: &str,
    requested: PacketBudgetModeDto,
) -> Option<Vec<String>> {
    let next = match requested {
        PacketBudgetModeDto::Tiny => "compact",
        PacketBudgetModeDto::Compact => "standard",
        PacketBudgetModeDto::Standard => "deep",
        PacketBudgetModeDto::Deep => return None,
    };
    let project = packet_display_project_arg(project_root);
    Some(packet_argv(&[
        "packet",
        "--project",
        project.as_str(),
        "--question",
        question,
        "--budget",
        next,
    ]))
}

/// The one-round DrillOnce continuation as executable argv. Budget stays the
/// parent's requested budget; the continuation is typed, not a deeper pass.
pub fn typed_drill_packet_argv(
    project_root: &Path,
    question: &str,
    requested: PacketBudgetModeDto,
    drill: &BoundedDrillPlanDto,
) -> Vec<String> {
    let project = packet_display_project_arg(project_root);
    let budget = packet_budget_cli_name(requested);
    let mut arguments = vec![
        "packet".to_string(),
        "--project".to_string(),
        project,
        "--question".to_string(),
        question.to_string(),
        "--budget".to_string(),
        budget.to_string(),
        "--parent-packet-id".to_string(),
        drill.parent_packet_id.clone(),
    ];
    for option in &drill.options {
        arguments.push("--option-id".to_string());
        arguments.push(option.id.clone());
    }
    if !drill.core_generation_id.is_empty() {
        arguments.push("--core-generation-id".to_string());
        arguments.push(drill.core_generation_id.clone());
    }
    if let Some(generation) = &drill.retrieval_generation {
        arguments.push("--retrieval-generation".to_string());
        arguments.push(generation.clone());
    }
    let refs = arguments.iter().map(String::as_str).collect::<Vec<_>>();
    packet_argv(&refs)
}

/// Prefer a typed DrillOnce continuation when the packet still has a remaining
/// round; otherwise fall back to the deeper-budget retry.
pub fn packet_follow_up_argv(
    project_root: &Path,
    question: &str,
    requested: PacketBudgetModeDto,
    drill: Option<&BoundedDrillPlanDto>,
) -> Option<Vec<String>> {
    if let Some(drill) = drill
        && drill.remaining_rounds > 0
        && !drill.options.is_empty()
    {
        return Some(typed_drill_packet_argv(
            project_root,
            question,
            requested,
            drill,
        ));
    }
    next_deeper_packet_argv(project_root, question, requested)
}

#[cfg(test)]
mod tests {
    use super::*;
    use codestory_contracts::api::{DrillOptionDto, PacketBudgetModeDto};
    use std::path::Path;

    fn sample_drill() -> BoundedDrillPlanDto {
        BoundedDrillPlanDto {
            parent_packet_id: "packet-1".to_string(),
            core_generation_id: "core-1".to_string(),
            retrieval_generation: Some("retrieval-1".to_string()),
            gap_ids: vec!["omitted-material:client_transport_send".to_string()],
            options: vec![DrillOptionDto::omitted_symbol(
                "omitted-material:client_transport_send",
                "symbol-1",
            )],
            max_bytes: 32 * 1024,
            max_hits: 8,
            max_depth: 2,
            remaining_rounds: 1,
        }
    }

    #[test]
    fn packet_argv_and_rendering_preserve_argv_and_quote_shell_values() {
        let argv = packet_argv(&[
            "packet",
            "--project",
            "/tmp/project with space",
            "--question",
            "it's ready",
        ]);

        assert_eq!(
            argv,
            vec![
                "codestory-cli".to_string(),
                "packet".to_string(),
                "--project".to_string(),
                "/tmp/project with space".to_string(),
                "--question".to_string(),
                "it's ready".to_string(),
            ],
        );
        assert_eq!(
            render_packet_command(&argv),
            "codestory-cli packet --project '/tmp/project with space' --question 'it'\\''s ready'",
        );
    }

    #[test]
    fn deeper_packet_argv_advances_budget_and_stops_at_deep() {
        let project = Path::new("/tmp/project with space");
        let argv = next_deeper_packet_argv(project, "show $HOME", PacketBudgetModeDto::Tiny)
            .expect("tiny has a deeper retry");

        assert_eq!(
            argv,
            vec![
                "codestory-cli".to_string(),
                "packet".to_string(),
                "--project".to_string(),
                "/tmp/project with space".to_string(),
                "--question".to_string(),
                "show $HOME".to_string(),
                "--budget".to_string(),
                "compact".to_string(),
            ],
        );
        assert_eq!(
            next_deeper_packet_argv(project, "question", PacketBudgetModeDto::Deep),
            None,
        );
    }

    #[test]
    fn typed_drill_argv_keeps_budget_and_forwards_pins() {
        let project = Path::new("/tmp/project with space");
        let drill = sample_drill();
        let argv =
            typed_drill_packet_argv(project, "show $HOME", PacketBudgetModeDto::Standard, &drill);

        assert_eq!(argv[0], "codestory-cli");
        assert_eq!(argv[1], "packet");
        assert!(argv.contains(&"--parent-packet-id".to_string()));
        assert!(argv.contains(&"packet-1".to_string()));
        assert!(argv.contains(&"--option-id".to_string()));
        assert!(argv.contains(&drill.options[0].id));
        assert!(argv.contains(&"--core-generation-id".to_string()));
        assert!(argv.contains(&"core-1".to_string()));
        assert!(argv.contains(&"--retrieval-generation".to_string()));
        assert!(argv.contains(&"retrieval-1".to_string()));
        assert!(argv.contains(&"--budget".to_string()));
        assert!(argv.contains(&"standard".to_string()));
        assert!(!argv.iter().any(|argument| argument == "deep"));
    }

    #[test]
    fn typed_drill_argv_omits_empty_generation_pins() {
        let mut drill = sample_drill();
        drill.core_generation_id.clear();
        drill.retrieval_generation = None;
        let argv = typed_drill_packet_argv(
            Path::new("/tmp/project"),
            "trace dispatch",
            PacketBudgetModeDto::Standard,
            &drill,
        );
        assert!(argv.contains(&"--parent-packet-id".to_string()));
        assert!(!argv.contains(&"--core-generation-id".to_string()));
        assert!(!argv.contains(&"--retrieval-generation".to_string()));
    }

    #[test]
    fn follow_up_argv_prefers_typed_drill_over_deeper_budget() {
        let project = Path::new("/tmp/project");
        let drill = sample_drill();
        let argv = packet_follow_up_argv(
            project,
            "trace dispatch",
            PacketBudgetModeDto::Compact,
            Some(&drill),
        )
        .expect("drill_once still has a continuation");
        assert!(argv.contains(&"--parent-packet-id".to_string()));
        assert!(argv.contains(&"compact".to_string()));
        assert!(!argv.iter().any(|argument| argument == "standard"));

        let mut spent = drill.clone();
        spent.remaining_rounds = 0;
        let deeper = packet_follow_up_argv(
            project,
            "trace dispatch",
            PacketBudgetModeDto::Compact,
            Some(&spent),
        )
        .expect("spent drill falls back to a deeper budget");
        assert!(deeper.contains(&"standard".to_string()));
        assert!(!deeper.contains(&"--parent-packet-id".to_string()));
    }
}
