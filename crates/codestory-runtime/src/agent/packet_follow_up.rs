use codestory_agent::packet_command::{next_deeper_packet_argv, render_packet_command};
use codestory_contracts::api::{AgentPacketDto, PacketFollowUpInvocationDto};
use std::collections::HashMap;
use std::path::Path;

const LOGICAL_CODESTORY_PROGRAM: &str = "codestory-cli";

/// Resolve logical packet follow-ups at the adapter boundary that knows which CodeStory
/// executable is actually serving the request. Structured invocations remain authoritative;
/// display commands are regenerated from them rather than parsed and rewritten.
pub fn bind_packet_follow_up_program(
    project_root: &Path,
    packet: &mut AgentPacketDto,
    executable: &Path,
) {
    let executable = executable.to_string_lossy().into_owned();
    let mut rendered_replacements = HashMap::new();
    for invocation in &mut packet.sufficiency.follow_up_invocations {
        let previous = render_follow_up_invocation(invocation);
        if invocation.program == LOGICAL_CODESTORY_PROGRAM {
            invocation.program.clone_from(&executable);
        }
        rendered_replacements.insert(previous, render_follow_up_invocation(invocation));
    }

    packet.sufficiency.follow_up_commands = packet
        .sufficiency
        .follow_up_invocations
        .iter()
        .map(render_follow_up_invocation)
        .collect();
    for open_next in &mut packet.sufficiency.open_next {
        if let Some(replacement) = rendered_replacements.get(open_next) {
            open_next.clone_from(replacement);
        }
    }

    packet.budget.next_deeper_command =
        next_deeper_packet_argv(project_root, &packet.question, packet.budget.requested).map(
            |mut argv| {
                if argv
                    .first()
                    .is_some_and(|program| program == LOGICAL_CODESTORY_PROGRAM)
                {
                    argv[0].clone_from(&executable);
                }
                render_packet_command(&argv)
            },
        );
}

fn render_follow_up_invocation(invocation: &PacketFollowUpInvocationDto) -> String {
    let mut argv = Vec::with_capacity(invocation.args.len() + 1);
    argv.push(invocation.program.clone());
    argv.extend(invocation.args.iter().cloned());
    render_packet_command(&argv)
}

#[cfg(test)]
mod tests {
    use super::*;
    use codestory_contracts::api::{PacketBudgetModeDto, PacketFollowUpInvocationDto};

    #[test]
    fn adapter_binding_rebuilds_every_command_from_the_structured_invocation() {
        let mut packet = super::super::packet_budget::tests::test_packet("trace routing", 98_304);
        let logical = PacketFollowUpInvocationDto {
            program: LOGICAL_CODESTORY_PROGRAM.to_string(),
            args: vec![
                "search".to_string(),
                "--project".to_string(),
                "/tmp/project with space".to_string(),
            ],
        };
        packet.sufficiency.follow_up_invocations = vec![logical];
        packet.sufficiency.follow_up_commands =
            vec!["codestory-cli search --project '/tmp/project with space'".to_string()];
        packet.sufficiency.open_next = packet.sufficiency.follow_up_commands.clone();
        packet.budget.requested = PacketBudgetModeDto::Compact;

        bind_packet_follow_up_program(
            Path::new("/tmp/project with space"),
            &mut packet,
            Path::new("/opt/CodeStory Managed/bin/codestory-cli"),
        );

        assert_eq!(
            packet.sufficiency.follow_up_invocations[0].program,
            "/opt/CodeStory Managed/bin/codestory-cli"
        );
        assert_eq!(
            packet.sufficiency.follow_up_commands,
            vec![
                "'/opt/CodeStory Managed/bin/codestory-cli' search --project '/tmp/project with space'"
            ]
        );
        assert_eq!(
            packet.sufficiency.open_next,
            packet.sufficiency.follow_up_commands
        );
        assert!(
            packet
                .budget
                .next_deeper_command
                .as_deref()
                .is_some_and(|command| command
                    .starts_with("'/opt/CodeStory Managed/bin/codestory-cli' packet"))
        );
    }
}
