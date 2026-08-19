use codestory_agent::packet_command::{packet_follow_up_argv, render_packet_command};
use codestory_contracts::api::AgentPacketDto;
use std::path::Path;

const LOGICAL_CODESTORY_PROGRAM: &str = "codestory-cli";

/// Bind the deeper-budget operator command at the adapter that knows which
/// executable is serving the request. Agent-facing disposition is not rewritten here.
pub fn bind_packet_follow_up_program(
    project_root: &Path,
    packet: &mut AgentPacketDto,
    executable: &Path,
) {
    let executable = executable.to_string_lossy().into_owned();
    packet.budget.next_deeper_command = packet_follow_up_argv(
        project_root,
        &packet.question,
        packet.budget.requested,
        packet.disposition.drill.as_ref(),
    )
    .map(|mut argv| {
        if argv
            .first()
            .is_some_and(|program| program == LOGICAL_CODESTORY_PROGRAM)
        {
            argv[0].clone_from(&executable);
        }
        render_packet_command(&argv)
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use codestory_contracts::api::{
        BoundedDrillPlanDto, DrillOptionDto, PacketBudgetModeDto, PacketDispositionDto,
        PacketDispositionKindDto,
    };
    use std::path::Path;

    #[test]
    fn adapter_binding_rewrites_the_deeper_packet_command_only() {
        let mut packet = super::super::packet_budget::tests::test_packet("trace routing", 98_304);
        packet.budget.requested = PacketBudgetModeDto::Compact;
        bind_packet_follow_up_program(
            Path::new("/tmp/project with space"),
            &mut packet,
            Path::new("/opt/CodeStory Managed/bin/codestory-cli"),
        );
        assert!(
            packet
                .budget
                .next_deeper_command
                .as_deref()
                .is_some_and(|command| command
                    .starts_with("'/opt/CodeStory Managed/bin/codestory-cli' packet"))
        );
        assert_eq!(
            packet.disposition.kind, packet.disposition.kind,
            "adapter binding must not reclassify disposition"
        );
    }

    #[test]
    fn adapter_binding_emits_typed_drill_argv_for_drill_once() {
        let mut packet = super::super::packet_budget::tests::test_packet("trace routing", 98_304);
        packet.budget.requested = PacketBudgetModeDto::Standard;
        packet.disposition = PacketDispositionDto::drill_once(
            "evidence is objectively missing and closable by the listed options",
            BoundedDrillPlanDto {
                parent_packet_id: packet.packet_id.clone(),
                core_generation_id: "core-1".to_string(),
                retrieval_generation: Some("retrieval-1".to_string()),
                gap_ids: vec!["omitted-edge:request_dispatch".to_string()],
                options: vec![DrillOptionDto::omitted_symbol(
                    "omitted-edge:request_dispatch",
                    "symbol-1",
                )],
                max_bytes: 32 * 1024,
                max_hits: 8,
                max_depth: 2,
                remaining_rounds: 1,
            },
        );
        bind_packet_follow_up_program(
            Path::new("/tmp/project"),
            &mut packet,
            Path::new("/opt/CodeStory Managed/bin/codestory-cli"),
        );
        let command = packet
            .budget
            .next_deeper_command
            .as_deref()
            .expect("drill_once must publish a typed continuation");
        assert!(command.contains("--parent-packet-id"));
        assert!(command.contains("--option-id"));
        assert!(command.contains("--core-generation-id"));
        assert!(!command.contains("--budget deep"));
        assert_eq!(packet.disposition.kind, PacketDispositionKindDto::DrillOnce);
    }
}
