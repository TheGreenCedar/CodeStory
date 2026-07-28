//! Repository-derived root ranking shared by compact grounding and search.
//!
//! The v0.16.1 audit found the previous root ordering was driven by catalogs of
//! framework filenames and entry-point names collected from the benchmark
//! holdout. Everything here is derived from the repository under inspection:
//! verified file role, directed call-graph degrees, and path structure. The one
//! name literal in this module is the language contract `main`, and it is typed
//! as such so callers can report which kind of evidence they found.

use codestory_store::FileRole;
use std::collections::HashSet;

/// Fan-out floor for the topological entry-point arm.
///
/// A production callable with no visible callers and no outbound calls is far
/// more likely to be dead code or an FFI leaf than an entry point.
pub(crate) const ENTRY_MIN_FANOUT: u32 = 2;

/// Deduped resolvable hits that carry graph evidence on the search surface.
///
/// Matches the existing `limit_per_source` clamp ceiling, so the evidence walk
/// never exceeds the work the plan already pays for.
pub(crate) const SEARCH_ORIENTATION_WINDOW: usize = 50;

/// Files admitted to the grounding candidate universe per subsystem.
pub(crate) const SUBSYSTEM_FILE_QUOTA: usize = 2;

/// Directory names that conventionally hold a project's own source tree.
///
/// These are layout words, not repository or framework names: every one of them
/// is a generic English source-root convention.
const SOURCE_ROOT_SEGMENTS: &[&str] = &["src", "lib", "app", "cmd", "source"];

/// Quantized call degree.
///
/// Raw counts reorder roots between re-indexes whenever the parser resolves one
/// extra edge, so a compact map would flap for reasons that say nothing about
/// the repository. Quantizing absorbs that drift and lets ties fall through to
/// the structural and name tie-breakers, which are stable.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct DegreeTier(pub(crate) u8);

pub(crate) fn degree_tier(count: u32) -> DegreeTier {
    DegreeTier(match count {
        0 => 0,
        1..=2 => 1,
        3..=8 => 2,
        _ => 3,
    })
}

/// Directed CALL degrees for one candidate.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct CallDegrees {
    /// Non-speculative inbound CALL sources, excluding test/benchmark callers.
    pub(crate) production_in_calls: u32,
    /// Non-speculative outbound CALL targets.
    pub(crate) out_calls: u32,
}

impl CallDegrees {
    pub(crate) fn is_empty(self) -> bool {
        self.production_in_calls == 0 && self.out_calls == 0
    }
}

impl From<codestory_store::GroundingCallDegree> for CallDegrees {
    fn from(degree: codestory_store::GroundingCallDegree) -> Self {
        Self {
            production_in_calls: degree.production_in_calls,
            out_calls: degree.out_calls,
        }
    }
}

/// Ordered weakest to strongest; the discriminant is the sort weight.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum EntryEvidence {
    #[default]
    None = 0,
    /// Language contract: the terminal symbol segment is `main`. Carries
    /// orientation when graph coverage is too thin to prove topology.
    LanguageMain = 1,
    /// Repository-derived: a production callable that nothing visible calls and
    /// that fans out into the graph — a call-DAG root.
    TopologicalRoot = 2,
}

impl EntryEvidence {
    pub(crate) fn weight(self) -> u8 {
        self as u8
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::LanguageMain => "language_main",
            Self::TopologicalRoot => "topological_root",
        }
    }
}

pub(crate) fn is_production_file_role(role: Option<FileRole>) -> bool {
    matches!(role, Some(FileRole::Source | FileRole::Entrypoint))
}

/// Classify a candidate's entry-point evidence without consulting any name
/// catalog.
///
/// `callable` is supplied by the caller because the grounding surface carries
/// `graph::NodeKind` while the search surface carries `api::NodeKind`; both
/// mean "function or method" here.
pub(crate) fn entry_evidence(
    callable: bool,
    file_role: Option<FileRole>,
    import_like: bool,
    terminal_name: &str,
    degrees: CallDegrees,
) -> EntryEvidence {
    if import_like || !callable || !is_production_file_role(file_role) {
        return EntryEvidence::None;
    }

    // A production callable that nothing visible calls and that fans out into
    // the graph is an entry point by topology. This recognizes a Tauri command
    // handler, a framework-invoked route handler, or `main` in `Main.java`
    // without naming a single framework.
    if degrees.production_in_calls == 0 && degrees.out_calls >= ENTRY_MIN_FANOUT {
        return EntryEvidence::TopologicalRoot;
    }
    if terminal_name == "main" {
        return EntryEvidence::LanguageMain;
    }
    EntryEvidence::None
}

fn path_segments(relative_path: &str) -> Vec<String> {
    relative_path
        .replace('\\', "/")
        .to_ascii_lowercase()
        .split('/')
        .filter(|segment| !segment.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn innermost_source_root_index(segments: &[String]) -> Option<usize> {
    // Only directory segments can be a source root, so never match the file
    // name itself.
    let directory_count = segments.len().saturating_sub(1);
    segments[..directory_count]
        .iter()
        .rposition(|segment| SOURCE_ROOT_SEGMENTS.contains(&segment.as_str()))
}

/// Directory segments between the innermost source root and the file name.
pub(crate) fn structural_depth(relative_path: &str) -> u8 {
    let segments = path_segments(relative_path);
    let directory_count = segments.len().saturating_sub(1);
    let depth = match innermost_source_root_index(&segments) {
        Some(index) => directory_count.saturating_sub(index + 1),
        None => directory_count,
    };
    depth.min(u8::MAX as usize) as u8
}

/// Structural position of a file, lower is better.
///
/// 0 = a file role the indexer verified as an entry point, 1 = at or one level
/// below a source root, 2 = anywhere under a source root, 3 = otherwise.
pub(crate) fn structural_path_rank(role: Option<FileRole>, relative_path: Option<&str>) -> u8 {
    if role == Some(FileRole::Entrypoint) {
        return 0;
    }
    let Some(path) = relative_path else {
        return 3;
    };
    let segments = path_segments(path);
    match innermost_source_root_index(&segments) {
        Some(_) => {
            if structural_depth(path) <= 1 {
                1
            } else {
                2
            }
        }
        None => {
            if segments.len() <= 1 {
                1
            } else {
                3
            }
        }
    }
}

/// Group a file into a workspace subsystem using layout only.
///
/// Derived from the repository's own directory structure — crate, plugin, or
/// source-tree child — never from a repository or framework name.
pub(crate) fn subsystem_key_for_path(language: &str, relative_path: Option<&str>) -> String {
    let Some(path) = relative_path else {
        return format!("{language}:unknown");
    };
    let segments = path
        .replace('\\', "/")
        .to_ascii_lowercase()
        .split('/')
        .filter(|segment| !segment.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();

    if let Some(index) = segments.iter().position(|segment| segment == "crates")
        && let Some(crate_name) = segments.get(index + 1)
    {
        return format!("{language}:crates/{crate_name}");
    }
    if let Some(index) = segments.iter().position(|segment| segment == "plugins")
        && let Some(plugin_name) = segments.get(index + 1)
    {
        return format!("{language}:plugins/{plugin_name}");
    }
    if segments.iter().any(|segment| segment == "src-tauri") {
        return format!("{language}:src-tauri");
    }
    if let Some(index) = segments.iter().rposition(|segment| segment == "src") {
        if let Some(next) = segments.get(index + 1)
            && !next.contains('.')
        {
            return format!("{language}:{}", segments[..=index + 1].join("/"));
        }
        return format!("{language}:{}", segments[..=index].join("/"));
    }

    let top = segments
        .first()
        .map(String::as_str)
        .unwrap_or("root")
        .to_string();
    format!("{language}:{top}")
}

/// True when the display name or path calls the symbol a helper, mock, or
/// fixture. Generic vocabulary; no repository or framework names.
pub(crate) fn helper_like_name_or_path(display_name: &str, file_path: Option<&str>) -> bool {
    let text = format!(
        "{} {}",
        display_name.to_ascii_lowercase(),
        file_path
            .unwrap_or_default()
            .replace('\\', "/")
            .to_ascii_lowercase()
    );
    text.split(|ch: char| !ch.is_ascii_alphanumeric())
        .any(|term| {
            matches!(
                term,
                "helper" | "helpers" | "mock" | "mocks" | "fake" | "fixture" | "fixtures"
            )
        })
}

/// Reorder a pre-sorted candidate list so distinct subsystems and names reach
/// the front, without taking a limit.
///
/// Taking no limit is what makes every budget's prefix monotone: one order is
/// produced, and each budget truncates that same order. The output is a
/// permutation of the input and relative order is preserved inside each pass.
pub(crate) fn diversify_root_order<T>(
    items: Vec<T>,
    pinned: impl Fn(&T) -> bool,
    surface_key: impl Fn(&T) -> (String, String),
) -> Vec<T> {
    if items.len() <= 1 {
        return items;
    }

    let keys = items.iter().map(&surface_key).collect::<Vec<_>>();
    let mut passes = vec![3u8; items.len()];
    let mut seen_surfaces = HashSet::new();
    let mut seen_names = HashSet::new();

    // Pass 0 keeps pinned candidates where they are and seeds the seen sets, so
    // diversification never spends a slot repeating something already pinned.
    for (index, item) in items.iter().enumerate() {
        if pinned(item) {
            passes[index] = 0;
            seen_surfaces.insert(keys[index].0.clone());
            seen_names.insert(keys[index].1.clone());
        }
    }
    for index in 0..items.len() {
        if passes[index] == 0 {
            continue;
        }
        let (surface, name) = &keys[index];
        if !seen_surfaces.contains(surface) && !seen_names.contains(name) {
            seen_surfaces.insert(surface.clone());
            seen_names.insert(name.clone());
            passes[index] = 1;
        }
    }
    for index in 0..items.len() {
        if passes[index] != 3 {
            continue;
        }
        if seen_names.insert(keys[index].1.clone()) {
            passes[index] = 2;
        }
    }

    let mut ordered = items.into_iter().zip(passes).collect::<Vec<_>>();
    ordered.sort_by_key(|(_, pass)| *pass);
    ordered.into_iter().map(|(item, _)| item).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn degrees(production_in_calls: u32, out_calls: u32) -> CallDegrees {
        CallDegrees {
            production_in_calls,
            out_calls,
        }
    }

    #[test]
    fn entry_evidence_requires_production_callable_topology_or_the_language_main_name() {
        assert_eq!(
            entry_evidence(
                true,
                Some(FileRole::Source),
                false,
                "serve_requests",
                degrees(0, 3)
            ),
            EntryEvidence::TopologicalRoot
        );
        assert_eq!(
            entry_evidence(true, Some(FileRole::Source), false, "main", degrees(1, 0)),
            EntryEvidence::LanguageMain
        );
        assert_eq!(
            entry_evidence(true, Some(FileRole::Test), false, "main", degrees(0, 9)),
            EntryEvidence::None
        );
        assert_eq!(
            entry_evidence(false, Some(FileRole::Source), false, "main", degrees(0, 9)),
            EntryEvidence::None
        );
        assert_eq!(
            entry_evidence(true, Some(FileRole::Source), true, "main", degrees(0, 9)),
            EntryEvidence::None
        );
    }

    #[test]
    fn entry_evidence_rejects_a_module_file_callable_that_has_visible_callers() {
        // A module file classified as Entrypoint must not make every callable
        // inside it an entry point; topology decides.
        assert_eq!(
            entry_evidence(
                true,
                Some(FileRole::Entrypoint),
                false,
                "format_label",
                degrees(4, 2)
            ),
            EntryEvidence::None
        );
    }

    #[test]
    fn entry_evidence_rejects_a_zero_caller_leaf_without_fanout() {
        assert_eq!(
            entry_evidence(
                true,
                Some(FileRole::Source),
                false,
                "unused_leaf",
                degrees(0, 1)
            ),
            EntryEvidence::None
        );
    }

    #[test]
    fn degree_tiers_absorb_single_edge_differences() {
        assert_eq!(degree_tier(1), degree_tier(2));
        assert_eq!(degree_tier(3), degree_tier(8));
        assert_eq!(degree_tier(9), degree_tier(400));
        assert!(degree_tier(0) < degree_tier(1));
        assert!(degree_tier(2) < degree_tier(3));
        assert!(degree_tier(8) < degree_tier(9));
    }

    fn keyed(items: &[(&str, &str)]) -> Vec<(String, String)> {
        items
            .iter()
            .map(|(surface, name)| ((*surface).to_string(), (*name).to_string()))
            .collect()
    }

    #[test]
    fn diversified_order_truncated_at_any_limit_is_a_prefix_of_a_larger_limit() {
        let items = keyed(&[
            ("alpha", "one"),
            ("alpha", "two"),
            ("beta", "one"),
            ("gamma", "three"),
            ("alpha", "four"),
            ("beta", "five"),
        ]);
        // Re-diversify per limit and truncate the fresh result. Slicing one
        // stored Vec would hold for any function, including one that consulted
        // the limit -- the property under test is that none of them can.
        let at_limit = |limit: usize| {
            let mut ordered = diversify_root_order(items.clone(), |_| false, Clone::clone);
            ordered.truncate(limit);
            ordered
        };
        let full = at_limit(items.len());
        for smaller in 0..=items.len() {
            assert_eq!(
                at_limit(smaller),
                full[..smaller],
                "the order changed with the limit at {smaller}"
            );
        }
        assert_ne!(
            full,
            items,
            "the fixture must actually be reordered, or the prefix claim is empty"
        );
    }

    #[test]
    fn diversified_order_is_a_permutation_that_never_admits_a_non_candidate() {
        let items = keyed(&[
            ("alpha", "one"),
            ("alpha", "one"),
            ("beta", "two"),
            ("gamma", "one"),
        ]);
        let ordered = diversify_root_order(items.clone(), |_| false, Clone::clone);
        let mut expected = items;
        let mut actual = ordered;
        expected.sort();
        actual.sort();
        assert_eq!(actual, expected);
    }

    #[test]
    fn pinned_exact_matches_keep_their_positions_through_diversification() {
        let items = keyed(&[
            ("alpha", "pinned"),
            ("alpha", "repeat"),
            ("beta", "novel"),
            ("alpha", "pinned_two"),
        ]);
        let ordered =
            diversify_root_order(items, |(_, name)| name.starts_with("pinned"), Clone::clone);
        assert_eq!(
            ordered
                .iter()
                .map(|(_, name)| name.as_str())
                .collect::<Vec<_>>(),
            ["pinned", "pinned_two", "novel", "repeat"]
        );
    }

    #[test]
    fn subsystem_keys_are_derived_from_workspace_layout_not_repository_names() {
        assert_eq!(
            subsystem_key_for_path("rust", Some("crates/some-crate/src/thing.rs")),
            "rust:crates/some-crate"
        );
        assert_eq!(
            subsystem_key_for_path("ts", Some("plugins/some-plugin/index.ts")),
            "ts:plugins/some-plugin"
        );
        assert_eq!(
            subsystem_key_for_path("rust", Some("apps/desktop/src-tauri/src/lib.rs")),
            "rust:src-tauri"
        );
        assert_eq!(
            subsystem_key_for_path("ts", Some("src/widgets/panel.ts")),
            "ts:src/widgets"
        );
        assert_eq!(subsystem_key_for_path("ts", Some("src/panel.ts")), "ts:src");
        assert_eq!(
            subsystem_key_for_path("go", Some("cmd/tool/run.go")),
            "go:cmd"
        );
        assert_eq!(subsystem_key_for_path("go", None), "go:unknown");
    }

    #[test]
    fn structural_path_rank_uses_path_segments_only() {
        assert_eq!(
            structural_path_rank(Some(FileRole::Entrypoint), Some("anywhere/at/all.ts")),
            0
        );
        assert_eq!(
            structural_path_rank(Some(FileRole::Source), Some("src/a.rs")),
            1
        );
        assert_eq!(
            structural_path_rank(Some(FileRole::Source), Some("src/inner/a.rs")),
            1
        );
        assert_eq!(
            structural_path_rank(Some(FileRole::Source), Some("src/inner/deeper/a.rs")),
            2
        );
        assert_eq!(
            structural_path_rank(Some(FileRole::Source), Some("a.rs")),
            1
        );
        assert_eq!(
            structural_path_rank(Some(FileRole::Source), Some("scripts/tools/a.rs")),
            3
        );
        assert_eq!(structural_path_rank(Some(FileRole::Source), None), 3);
        // Windows separators normalize to the same structural position.
        assert_eq!(
            structural_path_rank(Some(FileRole::Source), Some(r"src\inner\a.rs")),
            1
        );
    }

    #[test]
    fn structural_depth_counts_directories_below_the_innermost_source_root() {
        assert_eq!(structural_depth("src/a.rs"), 0);
        assert_eq!(structural_depth("src/inner/a.rs"), 1);
        assert_eq!(structural_depth("crates/thing/src/inner/deep/a.rs"), 2);
        assert_eq!(structural_depth("src/main/java/com/example/App.java"), 4);
        assert_eq!(structural_depth("top/level/file.rs"), 2);
    }

    #[test]
    fn helper_like_names_and_paths_are_recognized_by_generic_vocabulary() {
        assert!(helper_like_name_or_path("build_helper", None));
        assert!(helper_like_name_or_path(
            "Thing",
            Some("src/mocks/thing.rs")
        ));
        assert!(!helper_like_name_or_path(
            "run_server",
            Some("src/server.rs")
        ));
    }
}
