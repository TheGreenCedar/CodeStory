//! Env-gated accuracy pipeline ledger for packet selection remediation.
//!
//! The public packet intentionally exposes only bounded evidence. When a material carrier goes
//! missing, that projection is too late to distinguish a retrieval miss from a protection,
//! source-window, or public-selection loss. This ledger records those boundaries in the existing
//! developer step-trace artifact without adding a product response field or changing packet
//! budgets.

use std::path::Path;

use codestory_agent::packet_flow_requirements::PacketMaterialFacetCarrier;
use codestory_contracts::{
    api::{
        AgentAnswerDto, AgentCitationDto, EdgeId, NodeId, PacketPlanDto, SupportUnitDto,
        SupportUnitKindDto,
    },
    packet_projection_v3::PacketEvidenceRowV3Dto,
};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

const LEDGER_KEY: &str = "accuracy_stage_ledger";
const LEDGER_SCHEMA: &str = "codestory.packet-accuracy-stage-ledger/v1";

pub(crate) fn record_pre_projection_stages_from_env(
    plan: &PacketPlanDto,
    pre_rank_citations: Option<&[AgentCitationDto]>,
    uncapped_citations: &[AgentCitationDto],
    selected_material_facets: &[PacketMaterialFacetCarrier],
    protected_node_ids: &[NodeId],
    protected_edge_ids: &[EdgeId],
) -> Option<String> {
    update_trace_from_env("pre_projection", |root| {
        let query_plan = plan
            .queries
            .iter()
            .enumerate()
            .map(|(index, query)| {
                json!({
                    "index": index,
                    "query": query.query,
                    "purpose": query.purpose,
                })
            })
            .collect::<Vec<_>>();
        let pre_rank_rows = pre_rank_citations
            .unwrap_or_default()
            .iter()
            .enumerate()
            .map(|(index, citation)| citation_row(index, citation))
            .collect::<Vec<_>>();
        let citations = uncapped_citations
            .iter()
            .enumerate()
            .map(|(index, citation)| citation_row(index, citation))
            .collect::<Vec<_>>();
        let protected_nodes = protected_node_ids
            .iter()
            .enumerate()
            .map(|(rank, node_id)| {
                let citation = uncapped_citations
                    .iter()
                    .find(|citation| citation.node_id == *node_id);
                json!({
                    "protection_rank": rank,
                    "node_id": node_id.0,
                    "citation": citation.map(|citation| citation_row(rank, citation)),
                })
            })
            .collect::<Vec<_>>();
        let material_facets = plan
            .obligations
            .claim_obligations
            .iter()
            .filter(|obligation| obligation.material)
            .map(|obligation| {
                let selected_node_ids = selected_material_facets
                    .iter()
                    .filter(|carrier| carrier.facet_id == obligation.id)
                    .map(|carrier| carrier.node_id.clone())
                    .collect::<Vec<_>>();
                json!({
                    "facet_id": obligation.id,
                    "kind": obligation.kind,
                    "binding_terms": obligation.binding_terms,
                    "carrier_node_ids": if selected_node_ids.is_empty() {
                        obligation.carrier_node_ids.clone()
                    } else {
                        selected_node_ids
                    },
                    "carrier_paths": obligation.carrier_paths,
                    "open_next_candidates": obligation.open_next_candidates,
                })
            })
            .collect::<Vec<_>>();
        root.insert(
            LEDGER_KEY.to_string(),
            json!({
                "schema_version": LEDGER_SCHEMA,
                "query_plan": {
                    "count": query_plan.len(),
                    "rows": query_plan,
                },
                "uncapped_citations": {
                    "pre_rank_count": pre_rank_rows.len(),
                    "pre_rank_rows": pre_rank_rows,
                    "count": citations.len(),
                    "rows": citations,
                },
                "protected_material_carriers": {
                    "node_count": protected_nodes.len(),
                    "edge_count": protected_edge_ids.len(),
                    "nodes": protected_nodes,
                    "edge_ids": protected_edge_ids.iter().map(|edge| edge.0.clone()).collect::<Vec<_>>(),
                    "facets": material_facets,
                    "selected_facets": selected_material_facets.iter().map(|carrier| json!({
                        "facet_id": carrier.facet_id,
                        "node_id": carrier.node_id.0,
                    })).collect::<Vec<_>>(),
                },
            }),
        );
    })
}

pub(crate) fn record_source_support_stage_from_env(
    answer: &AgentAnswerDto,
    support: &[SupportUnitDto],
) -> Option<String> {
    update_trace_from_env("source_support", |root| {
        let ledger = ensure_ledger(root);
        let post_cap_citations = answer
            .citations
            .iter()
            .enumerate()
            .map(|(index, citation)| citation_row(index, citation))
            .collect::<Vec<_>>();
        let windows = support
            .iter()
            .filter(|unit| unit.kind == SupportUnitKindDto::SourceRange)
            .map(source_support_row)
            .collect::<Vec<_>>();
        ledger.insert(
            "source_support_windows".to_string(),
            json!({
                "post_cap_citation_count": post_cap_citations.len(),
                "post_cap_citations": post_cap_citations,
                "window_count": windows.len(),
                "windows": windows,
            }),
        );
    })
}

pub(crate) fn record_public_projection_stage_from_env(
    evidence: &[PacketEvidenceRowV3Dto],
) -> Option<String> {
    update_trace_from_env("public_v3_evidence", |root| {
        let ledger = ensure_ledger(root);
        ledger.insert(
            "public_v3_evidence".to_string(),
            json!({
                "count": evidence.len(),
                "rows": evidence,
            }),
        );
    })
}

/// Preserve the pipeline ledger when the ordinary step-trace writer fills the rest of the same
/// artifact after the pre-projection stages have already been recorded.
pub(crate) fn restore_existing_ledger(trace_path: &Path, trace: &mut Value) {
    let Some(existing) = read_trace_root(trace_path) else {
        return;
    };
    let Some(ledger) = existing.get(LEDGER_KEY).cloned() else {
        return;
    };
    trace[LEDGER_KEY] = ledger;
}

fn citation_row(index: usize, citation: &AgentCitationDto) -> Value {
    json!({
        "index": index,
        "node_id": citation.node_id.0,
        "display_name": citation.display_name,
        "path": citation.file_path,
        "line": citation.line,
        "kind": citation.kind,
        "origin": citation.origin,
        "evidence_tier": citation.evidence_tier,
        "resolution_status": citation.resolution_status,
        "eligible_for_sufficiency": citation.eligible_for_sufficiency,
        "loss_reason": citation.loss_reason,
    })
}

fn source_support_row(unit: &SupportUnitDto) -> Value {
    let snippet = unit.snippet.as_deref().unwrap_or_default();
    json!({
        "id": unit.id,
        "path": unit.path,
        "symbol_id": unit.symbol_id,
        "start_line": unit.start_line,
        "end_line": unit.end_line,
        "snippet_bytes": snippet.len(),
        "snippet_sha256": digest_hex(snippet.as_bytes()),
    })
}

fn digest_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn ensure_ledger(root: &mut Map<String, Value>) -> &mut Map<String, Value> {
    let ledger = root
        .entry(LEDGER_KEY.to_string())
        .or_insert_with(|| json!({ "schema_version": LEDGER_SCHEMA }));
    if !ledger.is_object() {
        *ledger = json!({ "schema_version": LEDGER_SCHEMA });
    }
    ledger
        .as_object_mut()
        .expect("accuracy stage ledger is an object")
}

fn update_trace_from_env(
    stage: &str,
    update: impl FnOnce(&mut Map<String, Value>),
) -> Option<String> {
    let trace_path =
        std::env::var(codestory_contracts::config_registry::PACKET_STEP_TRACE_OUT_ENV).ok()?;
    let path = Path::new(&trace_path);
    let mut root = read_trace_root(path).unwrap_or_else(|| Value::Object(Map::new()));
    if !root.is_object() {
        root = Value::Object(Map::new());
    }
    update(root.as_object_mut().expect("trace root is an object"));
    let payload = match serde_json::to_vec_pretty(&root) {
        Ok(payload) => payload,
        Err(error) => {
            return Some(format!(
                "packet_accuracy_stage_ledger error=serialize stage={stage} path={trace_path} message={error}"
            ));
        }
    };
    if let Err(error) = std::fs::write(path, payload) {
        return Some(format!(
            "packet_accuracy_stage_ledger error=write stage={stage} path={trace_path} message={error}"
        ));
    }
    None
}

fn read_trace_root(path: &Path) -> Option<Value> {
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_and_public_stages_extend_the_same_five_stage_ledger() {
        let mut root = Map::new();
        root.insert(
            LEDGER_KEY.to_string(),
            json!({
                "schema_version": LEDGER_SCHEMA,
                "query_plan": {"count": 1, "rows": []},
                "uncapped_citations": {"count": 1, "rows": []},
                "protected_material_carriers": {"node_count": 1, "nodes": []},
            }),
        );

        let ledger = ensure_ledger(&mut root);
        ledger.insert(
            "source_support_windows".to_string(),
            json!({"window_count": 1, "windows": []}),
        );
        ledger.insert(
            "public_v3_evidence".to_string(),
            json!({"count": 1, "rows": []}),
        );

        let ledger = root.get(LEDGER_KEY).expect("ledger");
        for stage in [
            "query_plan",
            "uncapped_citations",
            "protected_material_carriers",
            "source_support_windows",
            "public_v3_evidence",
        ] {
            assert!(ledger.get(stage).is_some(), "missing stage {stage}");
        }
        assert_eq!(ledger["schema_version"], LEDGER_SCHEMA);
    }

    #[test]
    fn restoring_the_ledger_does_not_replace_the_step_trace() {
        let root = std::env::temp_dir().join(format!(
            "codestory-accuracy-ledger-{}-{}.json",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        std::fs::write(
            &root,
            serde_json::to_vec(&json!({
                LEDGER_KEY: {"schema_version": LEDGER_SCHEMA, "query_plan": {"count": 2}}
            }))
            .unwrap(),
        )
        .unwrap();
        let mut trace = json!({"step_count": 7});

        restore_existing_ledger(&root, &mut trace);

        assert_eq!(trace["step_count"], 7);
        assert_eq!(trace[LEDGER_KEY]["query_plan"]["count"], 2);
        let _ = std::fs::remove_file(root);
    }
}
