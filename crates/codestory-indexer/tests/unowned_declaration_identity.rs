//! Unowned declaration identity across position-shifting edits (SRC-C2).
//!
//! SRC-C gave *callables* a position-independent identity. Everything else a
//! file declares — its imports, its top-level constants, its fields, its class
//! and namespace headers — is "unowned": no callable projection row repairs it,
//! so the file-structural fence used to hash every one of those rows together
//! with its span. Inserting a single header comment moved all of them, the
//! fence read that as churn, and `FullReplace` deleted every `bookmark_node`
//! row in the file.
//!
//! Every assertion here reads rows the production incremental path actually
//! wrote, through `WorkspaceIndexer::run_incremental` against a real store. The
//! central oracle is stronger than "the bookmark is still there": the store an
//! incremental refresh leaves behind must be **byte-identical** to the store a
//! from-scratch index of the same post-edit source produces. That is what rules
//! out the failure modes a bookmark count cannot see — a stale occurrence at
//! the pre-shift span, an edge still carrying its old line, an abandoned node
//! row.

use codestory_contracts::events::EventBus;
use codestory_contracts::graph::{NodeId, NodeKind};
use codestory_indexer::WorkspaceIndexer;
use codestory_store::Store as Storage;
use rusqlite::types::Value;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use tempfile::tempdir;

fn reindex(root: &Path, storage: &mut Storage, path: &Path) -> anyhow::Result<()> {
    let indexer = WorkspaceIndexer::new(root.to_path_buf());
    let event_bus = EventBus::new();
    let refresh_info = codestory_workspace::RefreshInfo {
        mode: codestory_workspace::BuildMode::Incremental,
        files_to_index: vec![path.to_path_buf()],
        files_to_remove: vec![],
        existing_file_ids: HashMap::new(),
    };
    indexer.run_incremental(storage, &refresh_info, &event_bus, None)?;
    Ok(())
}

/// Every projection table whose rows the reposition repair is responsible for,
/// plus the node table it must leave alone.
///
/// `file` is excluded on purpose: it carries the source mtime, which two runs
/// separated by a write cannot agree on and which no consumer of this contract
/// reads.
const PROJECTION_QUERIES: [(&str, &str); 5] = [
    (
        "node",
        "SELECT id, kind, serialized_name, qualified_name, canonical_id, file_node_id,
                start_line, start_col, end_line, end_col
         FROM node ORDER BY id",
    ),
    (
        "edge",
        "SELECT id, source_node_id, target_node_id, kind, file_node_id, line,
                resolved_source_node_id, resolved_target_node_id, callsite_identity
         FROM edge ORDER BY id, source_node_id, target_node_id, kind",
    ),
    (
        "occurrence",
        "SELECT element_id, kind, file_node_id, start_line, start_col, end_line, end_col
         FROM occurrence
         ORDER BY element_id, kind, file_node_id, start_line, start_col, end_line, end_col",
    ),
    (
        "callable_projection_state",
        "SELECT file_id, symbol_key, node_id, signature_hash, normalized_signature,
                body_hash, start_line, end_line
         FROM callable_projection_state ORDER BY file_id, symbol_key",
    ),
    (
        "component_access",
        "SELECT node_id, type FROM component_access ORDER BY node_id, type",
    ),
];

/// One named table's rows, each row a list of column values.
type TableRows = (&'static str, Vec<Vec<Value>>);

fn projection_snapshot(storage: &Storage) -> anyhow::Result<Vec<TableRows>> {
    let mut snapshot = Vec::with_capacity(PROJECTION_QUERIES.len());
    for (name, query) in PROJECTION_QUERIES {
        let mut statement = storage.get_connection().prepare(query)?;
        let column_count = statement.column_count();
        let rows = statement
            .query_map([], |row| {
                (0..column_count)
                    .map(|column| row.get::<_, Value>(column))
                    .collect::<rusqlite::Result<Vec<_>>>()
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        snapshot.push((name, rows));
    }
    Ok(snapshot)
}

fn assert_repaired_like_a_fresh_index(
    repaired: &Storage,
    fresh: &Storage,
    label: &str,
) -> anyhow::Result<()> {
    for ((table, repaired_rows), (_, fresh_rows)) in projection_snapshot(repaired)?
        .into_iter()
        .zip(projection_snapshot(fresh)?)
    {
        assert_eq!(
            repaired_rows, fresh_rows,
            "[{label}] the incrementally repaired `{table}` projection differs from a \
             from-scratch index of the same source"
        );
    }
    Ok(())
}

fn node_named(storage: &Storage, name: &str) -> anyhow::Result<Option<(NodeId, Option<u32>)>> {
    let matches = storage
        .get_nodes()?
        .into_iter()
        .filter(|node| node.serialized_name == name)
        .collect::<Vec<_>>();
    assert!(
        matches.len() <= 1,
        "expected at most one `{name}`, got {:?}",
        matches
            .iter()
            .map(|node| (node.id.0, node.start_line))
            .collect::<Vec<_>>()
    );
    Ok(matches.first().map(|node| (node.id, node.start_line)))
}

fn file_node_id(storage: &Storage) -> anyhow::Result<NodeId> {
    Ok(storage
        .get_nodes()?
        .into_iter()
        .find(|node| node.kind == NodeKind::FILE)
        .expect("file node")
        .id)
}

/// One language family's fixture.
struct LanguageCase {
    label: &'static str,
    file_name: &'static str,
    /// Source declaring an import, a top-level (or class-level) constant, a
    /// type header, and one callable.
    source: &'static str,
    /// The callable whose bookmark must survive.
    callable: &'static str,
    /// The type header that only moves.
    header: &'static str,
    /// An unowned declaration node that disappears in `without_import`.
    import_node: &'static str,
    /// The same source with the import removed: a genuine population change
    /// that must still take the whole-file replacement.
    without_import: &'static str,
    /// Comment syntax for the inserted header line.
    line_comment: &'static str,
    /// Line the comment is inserted *before* (1-based), for languages whose
    /// first line must stay first.
    insert_before_line: usize,
}

const CASES: &[LanguageCase] = &[
    LanguageCase {
        label: "java",
        file_name: "Widget.java",
        source: "import java.util.List;\n\npublic class Widget {\n    public static final int LIMIT = 10;\n\n    public int total(int base) {\n        return base + LIMIT;\n    }\n}\n",
        callable: "Widget.total",
        header: "Widget",
        import_node: "java.util.List",
        without_import: "public class Widget {\n    public static final int LIMIT = 10;\n\n    public int total(int base) {\n        return base + LIMIT;\n    }\n}\n",
        line_comment: "//",
        insert_before_line: 1,
    },
    LanguageCase {
        label: "csharp",
        file_name: "Widget.cs",
        source: "using System;\n\nnamespace Demo\n{\n    public class Widget\n    {\n        public const int Limit = 10;\n\n        public int Total(int baseValue)\n        {\n            return baseValue + Limit;\n        }\n    }\n}\n",
        callable: "Widget.Total",
        header: "Widget",
        import_node: "System",
        without_import: "namespace Demo\n{\n    public class Widget\n    {\n        public const int Limit = 10;\n\n        public int Total(int baseValue)\n        {\n            return baseValue + Limit;\n        }\n    }\n}\n",
        line_comment: "//",
        insert_before_line: 1,
    },
    LanguageCase {
        label: "kotlin",
        file_name: "Widget.kt",
        source: "import kotlin.math.max\n\nconst val LIMIT = 10\n\nclass Widget {\n    fun total(base: Int): Int {\n        return max(base, LIMIT)\n    }\n}\n",
        callable: "Widget.total",
        header: "Widget",
        import_node: "kotlin.math.max",
        without_import: "const val LIMIT = 10\n\nclass Widget {\n    fun total(base: Int): Int {\n        return base + LIMIT\n    }\n}\n",
        line_comment: "//",
        insert_before_line: 1,
    },
    LanguageCase {
        label: "python",
        file_name: "widget.py",
        source: "import os\n\nLIMIT = 10\n\n\nclass Widget:\n    def total(self, base):\n        return base + LIMIT\n",
        callable: "Widget.total",
        header: "Widget",
        import_node: "os",
        without_import: "LIMIT = 10\n\n\nclass Widget:\n    def total(self, base):\n        return base + LIMIT\n",
        line_comment: "#",
        insert_before_line: 1,
    },
    LanguageCase {
        label: "typescript",
        file_name: "widget.ts",
        source: "import { readFileSync } from \"fs\";\n\nexport const LIMIT = 10;\n\nexport class Widget {\n    total(base: number): number {\n        return base + LIMIT;\n    }\n}\n",
        callable: "Widget.total",
        header: "Widget",
        import_node: "\"fs\"",
        without_import: "export const LIMIT = 10;\n\nexport class Widget {\n    total(base: number): number {\n        return base + LIMIT;\n    }\n}\n",
        line_comment: "//",
        insert_before_line: 1,
    },
    LanguageCase {
        label: "ruby",
        file_name: "widget.rb",
        source: "require 'set'\n\nLIMIT = 10\n\nclass Widget\n  def total(base)\n    base + LIMIT\n  end\nend\n",
        callable: "Widget.total",
        header: "Widget",
        import_node: "'set'",
        without_import: "LIMIT = 10\n\nclass Widget\n  def total(base)\n    base + LIMIT\n  end\nend\n",
        line_comment: "#",
        insert_before_line: 1,
    },
    LanguageCase {
        label: "php",
        file_name: "widget.php",
        // `<?php` must stay on line 1, so the comment lands under it.
        source: "<?php\nnamespace Demo;\n\nconst LIMIT = 10;\n\nclass Widget\n{\n    public function total(int $base): int\n    {\n        return $base + LIMIT;\n    }\n}\n",
        callable: "Widget.total",
        header: "Widget",
        import_node: "Demo",
        without_import: "<?php\nconst LIMIT = 10;\n\nclass Widget\n{\n    public function total(int $base): int\n    {\n        return $base + LIMIT;\n    }\n}\n",
        line_comment: "//",
        insert_before_line: 2,
    },
    LanguageCase {
        label: "cpp",
        file_name: "widget.cpp",
        source: "#include <vector>\n\nconst int kLimit = 10;\n\nclass Widget {\npublic:\n    int total(int base) {\n        return base + kLimit;\n    }\n};\n",
        callable: "Widget::total",
        header: "Widget",
        import_node: "<vector>",
        without_import: "const int kLimit = 10;\n\nclass Widget {\npublic:\n    int total(int base) {\n        return base + kLimit;\n    }\n};\n",
        line_comment: "//",
        insert_before_line: 1,
    },
    LanguageCase {
        label: "go",
        file_name: "widget.go",
        source: "package demo\n\nimport \"fmt\"\n\nconst Limit = 10\n\nfunc Total(base int) string {\n\treturn fmt.Sprint(base + Limit)\n}\n",
        callable: "Total",
        header: "demo",
        import_node: "\"fmt\"",
        without_import: "package demo\n\nconst Limit = 10\n\nfunc Total(base int) int {\n\treturn base + Limit\n}\n",
        line_comment: "//",
        insert_before_line: 1,
    },
    LanguageCase {
        label: "rust",
        file_name: "widget.rs",
        source: "use std::fmt::Debug;\n\npub const LIMIT: i32 = 10;\n\npub struct Widget {\n    pub value: i32,\n}\n\npub fn total(base: i32) -> i32 {\n    base + LIMIT\n}\n",
        callable: "total",
        header: "Widget",
        import_node: "std::fmt::Debug",
        without_import: "pub const LIMIT: i32 = 10;\n\npub struct Widget {\n    pub value: i32,\n}\n\npub fn total(base: i32) -> i32 {\n    base + LIMIT\n}\n",
        line_comment: "//",
        insert_before_line: 1,
    },
];

impl LanguageCase {
    /// The same source with one comment line inserted, so every declaration
    /// below it — import, constant, type header, callable — moves down by one
    /// and nothing else changes.
    fn shifted(&self) -> String {
        let mut lines = self.source.lines().collect::<Vec<_>>();
        let comment = format!("{} shifted by SRC-C2", self.line_comment);
        lines.insert(self.insert_before_line - 1, comment.as_str());
        let mut out = lines.join("\n");
        out.push('\n');
        out
    }
}

#[test]
fn a_header_comment_preserves_bookmarks_in_every_language_family() -> anyhow::Result<()> {
    for case in CASES {
        let dir = tempdir()?;
        let root = dir.path();
        let path = root.join(case.file_name);
        fs::write(&path, case.source)?;
        let mut storage = Storage::new_in_memory()?;
        reindex(root, &mut storage, &path)?;

        let (callable_id, callable_line) = node_named(&storage, case.callable)?
            .unwrap_or_else(|| panic!("[{}] no `{}` node", case.label, case.callable));
        let (header_id, header_line) = node_named(&storage, case.header)?
            .unwrap_or_else(|| panic!("[{}] no `{}` node", case.label, case.header));
        let (import_id, import_line) = node_named(&storage, case.import_node)?
            .unwrap_or_else(|| panic!("[{}] no `{}` node", case.label, case.import_node));
        let node_ids_before = {
            let mut ids = storage
                .get_nodes()?
                .into_iter()
                .map(|node| node.id.0)
                .collect::<Vec<_>>();
            ids.sort_unstable();
            ids
        };

        let category = storage.create_bookmark_category("review")?;
        let on_callable = storage.add_bookmark(category, callable_id, Some("the callable"))?;
        let on_header = storage.add_bookmark(category, header_id, Some("the type header"))?;
        let on_import = storage.add_bookmark(category, import_id, Some("the import"))?;

        fs::write(&path, case.shifted())?;
        reindex(root, &mut storage, &path)?;

        let mut bookmarks = storage.get_bookmarks(Some(category))?;
        bookmarks.sort_by_key(|bookmark| bookmark.id);
        assert_eq!(
            bookmarks
                .iter()
                .map(|bookmark| (bookmark.id, bookmark.node_id))
                .collect::<Vec<_>>(),
            vec![
                (on_callable, callable_id),
                (on_header, header_id),
                (on_import, import_id),
            ],
            "[{}] inserting a header comment must not delete annotations anchored to \
             a callable, a type header, or an import",
            case.label
        );

        // Preserved is not enough: the rows have to have been *repaired*.
        assert_eq!(
            node_named(&storage, case.callable)?,
            Some((callable_id, callable_line.map(|line| line + 1))),
            "[{}] the callable must keep its id and follow the shift",
            case.label
        );
        assert_eq!(
            node_named(&storage, case.header)?,
            Some((header_id, header_line.map(|line| line + 1))),
            "[{}] the type header must keep its id and follow the shift",
            case.label
        );
        assert_eq!(
            node_named(&storage, case.import_node)?,
            Some((import_id, import_line.map(|line| line + 1))),
            "[{}] the import must keep its id and follow the shift",
            case.label
        );
        let node_ids_after = {
            let mut ids = storage
                .get_nodes()?
                .into_iter()
                .map(|node| node.id.0)
                .collect::<Vec<_>>();
            ids.sort_unstable();
            ids
        };
        assert_eq!(
            node_ids_before, node_ids_after,
            "[{}] a pure shift must neither mint nor abandon a node",
            case.label
        );

        // The whole projection, not just the rows this test happens to name.
        let mut fresh = Storage::new_in_memory()?;
        reindex(root, &mut fresh, &path)?;
        assert_repaired_like_a_fresh_index(&storage, &fresh, case.label)?;
    }
    Ok(())
}

#[test]
fn removing_an_unowned_declaration_still_replaces_the_file() -> anyhow::Result<()> {
    // Fail-closed control. The reposition repair deletes edges and occurrences
    // but never a node row, so it is only sound while the unowned population is
    // unchanged. An edit that *removes* an unowned declaration — and shifts
    // everything below it, so it looks like a shift to any position-blind
    // check — must still take the whole-file replacement, or the removed node
    // survives as an orphan that nothing will ever delete.
    for case in CASES {
        let dir = tempdir()?;
        let root = dir.path();
        let path = root.join(case.file_name);
        fs::write(&path, case.source)?;
        let mut storage = Storage::new_in_memory()?;
        reindex(root, &mut storage, &path)?;
        assert!(
            node_named(&storage, case.import_node)?.is_some(),
            "[{}] fixture must declare `{}`",
            case.label,
            case.import_node
        );

        fs::write(&path, case.without_import)?;
        reindex(root, &mut storage, &path)?;

        assert_eq!(
            node_named(&storage, case.import_node)?,
            None,
            "[{}] a removed unowned declaration must not survive an incremental refresh",
            case.label
        );

        let mut fresh = Storage::new_in_memory()?;
        reindex(root, &mut fresh, &path)?;
        assert_repaired_like_a_fresh_index(&storage, &fresh, case.label)?;
    }
    Ok(())
}

#[test]
fn a_shifted_import_leaves_no_stale_occurrence_or_edge_line() -> anyhow::Result<()> {
    // The two row shapes the fence exists for, read directly: occurrence rows
    // carry no id (a moved one becomes a second row rather than an updated
    // one), and non-call edge rows are insert-or-ignore on an id that does not
    // include the line (a moved one keeps its old line forever).
    let case = &CASES[0];
    let dir = tempdir()?;
    let root = dir.path();
    let path = root.join(case.file_name);
    fs::write(&path, case.source)?;
    let mut storage = Storage::new_in_memory()?;
    reindex(root, &mut storage, &path)?;

    let file_id = file_node_id(&storage)?;
    let before = storage.get_occurrences_for_file(file_id)?.len();
    let import_edge_lines_before = storage
        .get_edges()?
        .into_iter()
        .filter(|edge| edge.kind == codestory_contracts::graph::EdgeKind::IMPORT)
        .map(|edge| edge.line)
        .collect::<Vec<_>>();
    assert!(
        !import_edge_lines_before.is_empty(),
        "fixture must record an import edge"
    );

    fs::write(&path, case.shifted())?;
    reindex(root, &mut storage, &path)?;

    let after = storage.get_occurrences_for_file(file_node_id(&storage)?)?;
    assert_eq!(
        after.len(),
        before,
        "a pure shift must not add an occurrence row: {:?}",
        after
            .iter()
            .map(|occ| (occ.element_id, occ.location.start_line))
            .collect::<Vec<_>>()
    );
    let import_edge_lines_after = storage
        .get_edges()?
        .into_iter()
        .filter(|edge| edge.kind == codestory_contracts::graph::EdgeKind::IMPORT)
        .map(|edge| edge.line)
        .collect::<Vec<_>>();
    assert_eq!(
        import_edge_lines_after,
        import_edge_lines_before
            .iter()
            .map(|line| line.map(|line| line + 1))
            .collect::<Vec<_>>(),
        "the import edge must carry its new line, not its old one"
    );
    Ok(())
}
