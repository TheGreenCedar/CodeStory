//! Packet plan query normalization and deduplication.

use codestory_contracts::api::PacketPlanDto;
use std::collections::HashSet;

pub fn dedupe_packet_plan_queries(plan: &mut PacketPlanDto) {
    let mut seen = HashSet::<String>::new();
    let mut deduped = Vec::with_capacity(plan.queries.len());
    for query in plan.queries.drain(..) {
        let key = query.query.trim().to_ascii_lowercase();
        if key.len() < 2 {
            deduped.push(query);
            continue;
        }
        if seen.insert(key) {
            deduped.push(query);
        }
    }
    plan.queries = deduped;
}

#[cfg(test)]
mod tests {
    use super::*;
    use codestory_contracts::api::{PacketPlanDto, PacketPlanQueryDto};

    #[test]
    fn dedupe_does_not_interpret_english_stop_words() {
        let mut plan = PacketPlanDto {
            queries: vec![
                PacketPlanQueryDto {
                    query: "extension host startup flow".to_string(),
                    purpose: "a".to_string(),
                },
                PacketPlanQueryDto {
                    query: "the extension host startup flow".to_string(),
                    purpose: "b".to_string(),
                },
            ],
            probe_resolutions: Vec::new(),
            trace: Vec::new(),
        };
        dedupe_packet_plan_queries(&mut plan);
        assert_eq!(plan.queries.len(), 2);
    }
}
