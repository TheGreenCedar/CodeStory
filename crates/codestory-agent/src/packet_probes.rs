use codestory_contracts::api::{
    PacketProbeDto, PacketProbeResolutionDto, PacketProbeResolutionStatusDto,
};

pub fn exact_packet_probe_paths(resolutions: &[PacketProbeResolutionDto]) -> Vec<String> {
    resolutions
        .iter()
        .filter(|resolution| {
            matches!(
                resolution.status,
                PacketProbeResolutionStatusDto::ExactPath
                    | PacketProbeResolutionStatusDto::ValidUncoveredPath
            )
        })
        .filter_map(|resolution| match &resolution.probe {
            PacketProbeDto::ExactPath { path } => {
                Some(resolution.path.clone().unwrap_or_else(|| path.clone()))
            }
            _ => None,
        })
        .collect()
}
