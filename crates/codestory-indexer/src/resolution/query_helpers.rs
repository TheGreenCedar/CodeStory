use std::collections::HashSet;

pub(super) fn import_alias_mismatch(source_name: &str, target_name: &str) -> bool {
    let source = source_name.trim();
    let target = target_name.trim();
    if source.is_empty() || target.is_empty() {
        return false;
    }

    let target_tail = target
        .rsplit("::")
        .next()
        .and_then(|segment| {
            segment
                .rsplit_once('.')
                .map(|(_, tail)| tail)
                .or(Some(segment))
        })
        .map(str::trim)
        .unwrap_or(target);

    source != target_tail && (target.contains("::") || target.contains('.'))
}

pub(super) fn sorted_scope_file_ids(
    caller_scope_file_ids: Option<&HashSet<i64>>,
) -> Option<Vec<i64>> {
    caller_scope_file_ids.map(|scope| {
        let mut ids = scope.iter().copied().collect::<Vec<_>>();
        ids.sort_unstable();
        ids
    })
}
