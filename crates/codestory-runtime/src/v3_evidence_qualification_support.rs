//! Sealed Q1 fixture path through the real dark v3 evidence builders.

use codestory_contracts::{
    api::{AgentPacketRequestDto, PacketBudgetModeDto},
    packet_projection_v3::{
        ContextProjectionV3Dto, ContextTargetV3Dto, DiagnosticsCapabilityV3Dto, IdentityTextV3,
        PacketProjectionV3Dto, PathTextV3, RetrievalStateDescriptorV3Dto, RetrievalStateV3Dto,
        SearchProjectionV3Dto,
    },
};

use crate::agent::{
    packet_execution_record_v3::{
        FinalizedPacketExecutionInputV3, PacketProfileV3, PacketRequestFingerprintV3,
        build_packet_execution_record_fixture_v3,
    },
    packet_projection_v3::{
        FinalizedContextProjectionInputV3, FinalizedSearchProjectionInputV3,
        build_context_projection_v3, build_packet_projection_v3, build_search_projection_v3,
    },
};

/// Typed evidence-only projections constructed by the production-dark record
/// and projection modules. Nothing here registers a runtime method or route.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvidenceOnlyProjectionFixturesV3 {
    pub packet: PacketProjectionV3Dto,
    pub context: ContextProjectionV3Dto,
    pub search: SearchProjectionV3Dto,
}

/// Exercise the real packet execution record plus packet/context/search
/// projectors. The caller supplies revision-native packet measurement so the
/// packet builder's own 16 KiB policy is exercised without reversing the
/// runtime-to-CLI dependency direction.
pub fn real_projection_fixtures(
    mut measure_packet: impl FnMut(&PacketProjectionV3Dto) -> Result<usize, ()>,
) -> Result<EvidenceOnlyProjectionFixturesV3, String> {
    let request = AgentPacketRequestDto {
        question: "sealed evidence-only conformance".to_owned(),
        budget: PacketBudgetModeDto::Standard,
        task_class: None,
        probes: Vec::new(),
        extra_probes: Vec::new(),
        latency_budget_ms: None,
        parent_packet_id: None,
        option_ids: Vec::new(),
        core_generation_id: None,
        retrieval_generation: None,
    };
    let retrieval = RetrievalStateDescriptorV3Dto {
        state: RetrievalStateV3Dto::Unavailable,
        generation_id: None,
    };
    let input = FinalizedPacketExecutionInputV3::new(
        identity("evidence-only-conformance")?,
        identity("evidence-only-request")?,
        PacketRequestFingerprintV3::from_current_request(&request, PacketProfileV3::Auto),
        Vec::new(),
        Vec::new(),
        None,
        retrieval.clone(),
        Vec::new(),
    );
    let record = build_packet_execution_record_fixture_v3(&input, false)
        .map_err(|error| format!("record: {error:?}"))?;
    let packet = build_packet_projection_v3(
        &record,
        DiagnosticsCapabilityV3Dto::Unavailable,
        &mut measure_packet,
    )
    .map_err(|error| format!("packet: {error:?}"))?;
    let (identity, publication) = match &packet {
        PacketProjectionV3Dto::Complete {
            identity,
            publication,
            ..
        }
        | PacketProjectionV3Dto::BudgetExceeded {
            identity,
            publication,
            ..
        } => (identity.clone(), publication.clone()),
    };
    let context = build_context_projection_v3(&FinalizedContextProjectionInputV3::new(
        identity.clone(),
        publication.clone(),
        retrieval.clone(),
        ContextTargetV3Dto {
            path: Some(
                PathTextV3::new("src/lib.rs").map_err(|error| format!("context path: {error}"))?,
            ),
            symbol_id: None,
        },
        Vec::new(),
        Vec::new(),
        None,
        DiagnosticsCapabilityV3Dto::Unavailable,
        Vec::new(),
    ))
    .map_err(|error| format!("context: {error:?}"))?;
    let search = build_search_projection_v3(&FinalizedSearchProjectionInputV3::new(
        identity,
        publication,
        retrieval,
        Vec::new(),
        Vec::new(),
        None,
        DiagnosticsCapabilityV3Dto::Unavailable,
        Vec::new(),
    ))
    .map_err(|error| format!("search: {error:?}"))?;

    Ok(EvidenceOnlyProjectionFixturesV3 {
        packet,
        context,
        search,
    })
}

fn identity(value: &str) -> Result<IdentityTextV3, String> {
    IdentityTextV3::new(value).map_err(|error| error.to_string())
}
