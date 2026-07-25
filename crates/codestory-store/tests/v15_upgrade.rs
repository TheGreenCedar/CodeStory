//! Upgrade from a real v0.15 database.
//!
//! `Store::open` runs migrations and propagates any failure, and it is on the normal product
//! path, so a defect here is not a degraded index — it is a hard failure to open the project.
//! Every 0.15 user takes this path exactly once, and nothing else in the suite builds a database
//! with the schema they actually have: the other tests start from the current schema, where the
//! first migration a 0.15 database hits (`qdrant_collection` -> `semantic_generation`) is already
//! a no-op.

#[path = "fixtures/v15_schema.rs"]
mod v15_schema;

use codestory_store::Store;
use rusqlite::Connection;
use v15_schema::V15_TABLE_STATEMENTS;

/// The schema version v0.15.0 shipped.
const V15_SCHEMA_VERSION: u32 = 21;

fn seed_v15_database(path: &std::path::Path) {
    let conn = Connection::open(path).expect("create a v0.15 database");
    conn.execute_batch("PRAGMA foreign_keys = ON;")
        .expect("enable foreign keys");
    for statement in V15_TABLE_STATEMENTS {
        conn.execute_batch(statement)
            .unwrap_or_else(|error| panic!("apply v0.15 statement: {error}\n{statement}"));
    }

    conn.execute(
        "INSERT INTO node (id, kind, serialized_name, qualified_name, canonical_id, \
         file_node_id, start_line, start_col, end_line, end_col) \
         VALUES (1, 1, 'seeded_symbol', 'crate::seeded_symbol', 'canonical-seed', NULL, 1, 0, 4, 1)",
        [],
    )
    .expect("seed a node");
    conn.execute(
        "INSERT INTO file (id, path, language, modification_time, indexed, complete, \
         line_count, file_role, content_hash) \
         VALUES (1, 'src/lib.rs', 'rust', 1700000000000, 1, 1, 4, 'source', 'seed-hash')",
        [],
    )
    .expect("seed a file");
    // The row whose column this upgrade renames.
    conn.execute(
        "INSERT INTO retrieval_index_manifest \
         (project_id, lexical_version, qdrant_collection, built_at_epoch_ms) \
         VALUES ('seeded-project', 'lexical-v1', 'codestory-seeded-collection', 1700000000000)",
        [],
    )
    .expect("seed a v0.15 retrieval manifest");

    conn.pragma_update(None, "user_version", V15_SCHEMA_VERSION)
        .expect("stamp the v0.15 schema version");
    drop(conn);
}

#[test]
fn a_v15_database_upgrades_in_place_and_keeps_its_rows() {
    let directory = tempfile::tempdir().expect("temporary store root");
    let path = directory.path().join("codestory.db");
    seed_v15_database(&path);

    let storage = Store::open(&path).expect("a v0.15 database must open");
    drop(storage);

    let conn = Connection::open(&path).expect("reopen the upgraded database");
    let version: u32 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .expect("read the upgraded schema version");
    assert!(
        version > V15_SCHEMA_VERSION,
        "opening a v0.15 database must advance its schema version, got {version}"
    );

    // The rename must carry the value across rather than dropping and recreating the column.
    let generation: String = conn
        .query_row(
            "SELECT semantic_generation FROM retrieval_index_manifest WHERE project_id = ?1",
            ["seeded-project"],
            |row| row.get(0),
        )
        .expect("the migrated manifest row survives");
    assert_eq!(generation, "codestory-seeded-collection");

    let columns = conn
        .prepare("SELECT name FROM pragma_table_info('retrieval_index_manifest')")
        .and_then(|mut statement| {
            statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()
        })
        .expect("read the upgraded manifest columns");
    assert!(
        !columns.iter().any(|column| column == "qdrant_collection"),
        "the retired column must be gone, got {columns:?}"
    );

    let symbol: String = conn
        .query_row("SELECT serialized_name FROM node WHERE id = 1", [], |row| {
            row.get(0)
        })
        .expect("seeded graph rows survive the upgrade");
    assert_eq!(symbol, "seeded_symbol");
    let file_path: String = conn
        .query_row("SELECT path FROM file WHERE id = 1", [], |row| row.get(0))
        .expect("seeded file rows survive the upgrade");
    assert_eq!(file_path, "src/lib.rs");
}

#[test]
fn upgrading_twice_is_a_no_op() {
    let directory = tempfile::tempdir().expect("temporary store root");
    let path = directory.path().join("codestory.db");
    seed_v15_database(&path);

    drop(Store::open(&path).expect("first upgrade"));
    // A second open must not re-run the rename and fail on the already-migrated column.
    drop(Store::open(&path).expect("reopening an upgraded database must succeed"));

    let conn = Connection::open(&path).expect("reopen");
    let generation: String = conn
        .query_row(
            "SELECT semantic_generation FROM retrieval_index_manifest WHERE project_id = ?1",
            ["seeded-project"],
            |row| row.get(0),
        )
        .expect("the manifest row is still intact");
    assert_eq!(generation, "codestory-seeded-collection");
}
