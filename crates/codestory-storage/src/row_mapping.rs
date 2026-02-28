use super::*;

pub(super) fn node_from_row(row: &Row) -> Result<Node, StorageError> {
    let kind_int: i32 = row.get(1)?;
    Ok(Node {
        id: NodeId(row.get(0)?),
        kind: NodeKind::try_from(kind_int)?,
        serialized_name: row.get(2)?,
        qualified_name: row.get(3)?,
        canonical_id: row.get(4)?,
        file_node_id: row.get::<_, Option<i64>>(5)?.map(NodeId),
        start_line: row.get(6)?,
        start_col: row.get(7)?,
        end_line: row.get(8)?,
        end_col: row.get(9)?,
    })
}

pub(super) fn edge_from_row(row: &Row) -> Result<Edge, StorageError> {
    let kind_int: i32 = row.get(3)?;
    let certainty = row
        .get::<_, Option<String>>(10)?
        .as_deref()
        .and_then(ResolutionCertainty::from_str);
    let candidate_targets =
        deserialize_candidate_targets(row.get::<_, Option<String>>(11)?.as_deref())?;
    Ok(Edge {
        id: codestory_core::EdgeId(row.get(0)?),
        source: NodeId(row.get(1)?),
        target: NodeId(row.get(2)?),
        kind: EdgeKind::try_from(kind_int)?,
        file_node_id: row.get::<_, Option<i64>>(4)?.map(NodeId),
        line: row.get(5)?,
        resolved_source: row.get::<_, Option<i64>>(6)?.map(NodeId),
        resolved_target: row.get::<_, Option<i64>>(7)?.map(NodeId),
        confidence: row.get(8)?,
        callsite_identity: row.get(9)?,
        certainty,
        candidate_targets,
    })
}

pub(super) fn occurrence_from_row(row: &Row) -> rusqlite::Result<Occurrence> {
    let kind_int: i32 = row.get(1)?;
    Ok(Occurrence {
        element_id: row.get(0)?,
        kind: OccurrenceKind::try_from(kind_int).unwrap_or(OccurrenceKind::UNKNOWN),
        location: codestory_core::SourceLocation {
            file_node_id: codestory_core::NodeId(row.get(2)?),
            start_line: row.get(3)?,
            start_col: row.get(4)?,
            end_line: row.get(5)?,
            end_col: row.get(6)?,
        },
    })
}

pub(super) fn certainty_db_value(certainty: Option<ResolutionCertainty>) -> Option<&'static str> {
    certainty.map(ResolutionCertainty::as_str)
}

pub(super) fn access_kind_db_value(access: AccessKind) -> i32 {
    match access {
        AccessKind::Public => 0,
        AccessKind::Protected => 1,
        AccessKind::Private => 2,
        AccessKind::Default => 3,
    }
}

pub(super) fn access_kind_from_db(value: i32) -> AccessKind {
    match value {
        1 => AccessKind::Protected,
        2 => AccessKind::Private,
        3 => AccessKind::Default,
        _ => AccessKind::Public,
    }
}
