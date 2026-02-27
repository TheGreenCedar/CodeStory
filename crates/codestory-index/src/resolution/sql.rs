use super::*;

pub(super) fn cleanup_stale_call_resolutions(
    conn: &rusqlite::Connection,
    flags: ResolutionFlags,
    policy: ResolutionPolicy,
    scope_context: &ScopeCallerContext,
) -> Result<()> {
    super::cleanup_stale_call_resolutions(conn, flags, policy, scope_context)
}

pub(super) fn unresolved_edges(
    conn: &rusqlite::Connection,
    kind: EdgeKind,
    scope_context: &ScopeCallerContext,
) -> Result<Vec<UnresolvedEdgeRow>> {
    super::unresolved_edges(conn, kind, scope_context)
}

pub(super) fn apply_resolution_updates(
    conn: &rusqlite::Connection,
    updates: &[ResolvedEdgeUpdate],
) -> Result<()> {
    super::apply_resolution_updates(conn, updates)
}
