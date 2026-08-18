use codestory_contracts::api::{PacketBudgetModeDto, PacketFollowUpInvocationDto};
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

#[cfg(test)]
mod tests {
    use super::*;
    use codestory_contracts::api::PacketBudgetModeDto;
    use std::path::Path;

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
}
