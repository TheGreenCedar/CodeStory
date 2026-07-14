use super::{Storage, StorageError};
use rusqlite::Row;
use serde::{Deserialize, Serialize};

const MANIFEST_SELECT: &str = "
    SELECT
        project_id,
        lexical_version,
        qdrant_collection,
        scip_revision,
        built_at_epoch_ms,
        disk_bytes,
        degraded_modes_json,
        embedding_backend,
        embedding_dim,
        sidecar_schema_version,
        sidecar_input_hash,
        sidecar_generation,
        projection_count,
        symbol_doc_count,
        dense_projection_count,
        semantic_policy_version,
        graph_artifact_hash,
        dense_reason_counts_json,
        precise_semantic_import_status,
        precise_semantic_import_reason,
        precise_semantic_import_revision,
        precise_semantic_import_producer
    FROM retrieval_index_manifest";

/// Manifest row describing retrieval sidecar freshness for one project id.
///
/// Full retrieval readiness requires this row to match the current sidecar
/// schema, input hash, artifact generation, and graph/search projection counts.
/// Degraded modes are recorded explicitly so callers can fail closed instead of
/// treating SQLite graph state as equivalent to fresh sidecars.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetrievalIndexManifest {
    pub project_id: String,
    pub lexical_version: String,
    pub qdrant_collection: String,
    pub scip_revision: Option<String>,
    pub built_at_epoch_ms: i64,
    pub disk_bytes: Option<i64>,
    pub degraded_modes_json: String,
    /// e.g. `llamacpp:bge-base-en-v1.5` or `hash-projection:768`.
    pub embedding_backend: Option<String>,
    pub embedding_dim: Option<i32>,
    /// Version of the sidecar input hash/generation contract.
    pub sidecar_schema_version: Option<i32>,
    /// Stable hash of all local inputs used to build lexical, Qdrant, and SCIP artifacts.
    pub sidecar_input_hash: Option<String>,
    /// Artifact generation id used for lexical/SCIP directories.
    pub sidecar_generation: Option<String>,
    /// Number of symbol projection rows included in the sidecar input hash.
    pub projection_count: Option<i64>,
    /// Number of graph-native symbol-search docs included in the sidecar input hash.
    pub symbol_doc_count: Option<i64>,
    /// Number of dense semantic anchors included in Qdrant.
    pub dense_projection_count: Option<i64>,
    pub semantic_policy_version: Option<String>,
    pub graph_artifact_hash: Option<String>,
    pub dense_reason_counts_json: Option<String>,
    pub precise_semantic_import_status: Option<String>,
    pub precise_semantic_import_reason: Option<String>,
    pub precise_semantic_import_revision: Option<String>,
    pub precise_semantic_import_producer: Option<String>,
}

impl Storage {
    /// Insert or replace the retrieval manifest for a project id.
    pub fn upsert_retrieval_index_manifest(
        &mut self,
        manifest: &RetrievalIndexManifest,
    ) -> Result<(), StorageError> {
        self.conn.execute(
            "INSERT INTO retrieval_index_manifest (
                project_id,
                lexical_version,
                qdrant_collection,
                scip_revision,
                built_at_epoch_ms,
                disk_bytes,
                degraded_modes_json,
                embedding_backend,
                embedding_dim,
                sidecar_schema_version,
                sidecar_input_hash,
                sidecar_generation,
                projection_count,
                symbol_doc_count,
                dense_projection_count,
                semantic_policy_version,
                graph_artifact_hash,
                dense_reason_counts_json,
                precise_semantic_import_status,
                precise_semantic_import_reason,
                precise_semantic_import_revision,
                precise_semantic_import_producer
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22)
            ON CONFLICT(project_id) DO UPDATE SET
                lexical_version = excluded.lexical_version,
                qdrant_collection = excluded.qdrant_collection,
                scip_revision = excluded.scip_revision,
                built_at_epoch_ms = excluded.built_at_epoch_ms,
                disk_bytes = excluded.disk_bytes,
                degraded_modes_json = excluded.degraded_modes_json,
                embedding_backend = excluded.embedding_backend,
                embedding_dim = excluded.embedding_dim,
                sidecar_schema_version = excluded.sidecar_schema_version,
                sidecar_input_hash = excluded.sidecar_input_hash,
                sidecar_generation = excluded.sidecar_generation,
                projection_count = excluded.projection_count,
                symbol_doc_count = excluded.symbol_doc_count,
                dense_projection_count = excluded.dense_projection_count,
                semantic_policy_version = excluded.semantic_policy_version,
                graph_artifact_hash = excluded.graph_artifact_hash,
                dense_reason_counts_json = excluded.dense_reason_counts_json,
                precise_semantic_import_status = excluded.precise_semantic_import_status,
                precise_semantic_import_reason = excluded.precise_semantic_import_reason,
                precise_semantic_import_revision = excluded.precise_semantic_import_revision,
                precise_semantic_import_producer = excluded.precise_semantic_import_producer",
            rusqlite::params![
                manifest.project_id,
                manifest.lexical_version,
                manifest.qdrant_collection,
                manifest.scip_revision,
                manifest.built_at_epoch_ms,
                manifest.disk_bytes,
                manifest.degraded_modes_json,
                manifest.embedding_backend,
                manifest.embedding_dim,
                manifest.sidecar_schema_version,
                manifest.sidecar_input_hash,
                manifest.sidecar_generation,
                manifest.projection_count,
                manifest.symbol_doc_count,
                manifest.dense_projection_count,
                manifest.semantic_policy_version,
                manifest.graph_artifact_hash,
                manifest.dense_reason_counts_json,
                manifest.precise_semantic_import_status,
                manifest.precise_semantic_import_reason,
                manifest.precise_semantic_import_revision,
                manifest.precise_semantic_import_producer,
            ],
        )?;
        Ok(())
    }

    /// Load the retrieval manifest for a project id, if one has been built.
    pub fn get_retrieval_index_manifest(
        &self,
        project_id: &str,
    ) -> Result<Option<RetrievalIndexManifest>, StorageError> {
        let mut stmt = self
            .conn
            .prepare(&format!("{MANIFEST_SELECT} WHERE project_id = ?1"))?;
        let mut rows = stmt.query(rusqlite::params![project_id])?;
        let Some(row) = rows.next()? else {
            return Ok(None);
        };
        Ok(Some(manifest_from_row(row)?))
    }

    /// Return every current retrieval manifest in this store.
    ///
    /// Retention scans use the complete set so a shared sidecar root never
    /// removes a generation still referenced by another project row.
    pub fn list_retrieval_index_manifests(
        &self,
    ) -> Result<Vec<RetrievalIndexManifest>, StorageError> {
        let mut stmt = self.conn.prepare(MANIFEST_SELECT)?;
        let rows = stmt.query_map([], manifest_from_row)?;
        let mut manifests = Vec::new();
        for row in rows {
            manifests.push(row?);
        }
        Ok(manifests)
    }

    /// Return Qdrant collection names referenced by stored retrieval manifests.
    pub fn list_retrieval_qdrant_collections(&self) -> Result<Vec<String>, StorageError> {
        let mut stmt = self
            .conn
            .prepare("SELECT DISTINCT qdrant_collection FROM retrieval_index_manifest")?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        let mut collections = Vec::new();
        for row in rows {
            collections.push(row?);
        }
        Ok(collections)
    }

    pub fn clear_retrieval_index_manifests(&mut self) -> Result<usize, StorageError> {
        let removed = self
            .conn
            .execute("DELETE FROM retrieval_index_manifest", [])?;
        Ok(removed)
    }

    /// Latest manifest `built_at_epoch_ms` per Qdrant collection (for retention ranking).
    pub fn list_retrieval_qdrant_collections_with_recency(
        &self,
    ) -> Result<Vec<(String, i64)>, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT qdrant_collection, MAX(built_at_epoch_ms)
             FROM retrieval_index_manifest
             GROUP BY qdrant_collection",
        )?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        let mut collections = Vec::new();
        for row in rows {
            collections.push(row?);
        }
        Ok(collections)
    }
}

fn manifest_from_row(row: &Row<'_>) -> rusqlite::Result<RetrievalIndexManifest> {
    Ok(RetrievalIndexManifest {
        project_id: row.get(0)?,
        lexical_version: row.get(1)?,
        qdrant_collection: row.get(2)?,
        scip_revision: row.get(3)?,
        built_at_epoch_ms: row.get(4)?,
        disk_bytes: row.get(5)?,
        degraded_modes_json: row.get(6)?,
        embedding_backend: row.get(7)?,
        embedding_dim: row.get(8)?,
        sidecar_schema_version: row.get(9)?,
        sidecar_input_hash: row.get(10)?,
        sidecar_generation: row.get(11)?,
        projection_count: row.get(12)?,
        symbol_doc_count: row.get(13)?,
        dense_projection_count: row.get(14)?,
        semantic_policy_version: row.get(15)?,
        graph_artifact_hash: row.get(16)?,
        dense_reason_counts_json: row.get(17)?,
        precise_semantic_import_status: row.get(18)?,
        precise_semantic_import_reason: row.get(19)?,
        precise_semantic_import_revision: row.get(20)?,
        precise_semantic_import_producer: row.get(21)?,
    })
}

#[cfg(test)]
mod tests {
    use super::Storage;
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn list_retrieval_qdrant_collections_with_recency_uses_latest_manifest() {
        let dir = tempdir().expect("tempdir");
        let db_path = dir.path().join("codestory.db");
        let mut storage = Storage::open(&db_path).expect("open storage");
        for (project_id, collection, built_at) in [
            ("proj_a", "codestory_shared", 10_i64),
            ("proj_b", "codestory_shared", 99_i64),
            ("proj_c", "codestory_other", 5_i64),
        ] {
            storage
                .upsert_retrieval_index_manifest(&RetrievalIndexManifest {
                    project_id: project_id.into(),
                    lexical_version: "v1".into(),
                    qdrant_collection: collection.into(),
                    scip_revision: None,
                    built_at_epoch_ms: built_at,
                    disk_bytes: None,
                    degraded_modes_json: "[]".into(),
                    embedding_backend: None,
                    embedding_dim: None,
                    sidecar_schema_version: None,
                    sidecar_input_hash: None,
                    sidecar_generation: None,
                    projection_count: None,
                    symbol_doc_count: None,
                    dense_projection_count: None,
                    semantic_policy_version: None,
                    graph_artifact_hash: None,
                    dense_reason_counts_json: None,
                    precise_semantic_import_status: None,
                    precise_semantic_import_reason: None,
                    precise_semantic_import_revision: None,
                    precise_semantic_import_producer: None,
                })
                .expect("upsert manifest");
        }
        let mut recency = storage
            .list_retrieval_qdrant_collections_with_recency()
            .expect("list recency");
        recency.sort_by(|left, right| left.0.cmp(&right.0));
        assert_eq!(
            recency,
            vec![
                ("codestory_other".to_string(), 5),
                ("codestory_shared".to_string(), 99),
            ]
        );
    }

    #[test]
    fn retrieval_manifest_round_trips_sidecar_generation_fields() {
        let dir = tempdir().expect("tempdir");
        let db_path = dir.path().join("codestory.db");
        let mut storage = Storage::open(&db_path).expect("open storage");
        let manifest = RetrievalIndexManifest {
            project_id: "proj".into(),
            lexical_version: "v1".into(),
            qdrant_collection: "codestory_proj_deadbeef".into(),
            scip_revision: Some("graph-1234".into()),
            built_at_epoch_ms: 123,
            disk_bytes: Some(456),
            degraded_modes_json: "[]".into(),
            embedding_backend: Some("onnx:bge".into()),
            embedding_dim: Some(768),
            sidecar_schema_version: Some(1),
            sidecar_input_hash: Some("deadbeefcafebabe".into()),
            sidecar_generation: Some("proj-deadbeefcafebabe".into()),
            projection_count: Some(99),
            symbol_doc_count: Some(120),
            dense_projection_count: Some(99),
            semantic_policy_version: Some("graph_first_v1".into()),
            graph_artifact_hash: Some("graph-hash".into()),
            dense_reason_counts_json: Some("{\"public_api\":99}".into()),
            precise_semantic_import_status: Some("fresh".into()),
            precise_semantic_import_reason: None,
            precise_semantic_import_revision: Some("imported-a".into()),
            precise_semantic_import_producer: Some("scip-fixture".into()),
        };
        storage
            .upsert_retrieval_index_manifest(&manifest)
            .expect("upsert manifest");

        let loaded = storage
            .get_retrieval_index_manifest("proj")
            .expect("load manifest")
            .expect("manifest exists");

        assert_eq!(loaded, manifest);
    }

    #[test]
    fn list_retrieval_index_manifests_returns_every_project_row() {
        let dir = tempdir().expect("tempdir");
        let db_path = dir.path().join("codestory.db");
        let mut storage = Storage::open(&db_path).expect("open storage");
        for (project_id, suffix) in [
            ("proj_a", "aaaaaaaaaaaaaaaa"),
            ("proj_b", "bbbbbbbbbbbbbbbb"),
        ] {
            storage
                .upsert_retrieval_index_manifest(&RetrievalIndexManifest {
                    project_id: project_id.into(),
                    lexical_version: "v1".into(),
                    qdrant_collection: format!("codestory_{project_id}_{suffix}"),
                    scip_revision: Some(format!("graph-{suffix}")),
                    built_at_epoch_ms: 1,
                    disk_bytes: None,
                    degraded_modes_json: "[]".into(),
                    embedding_backend: None,
                    embedding_dim: None,
                    sidecar_schema_version: Some(2),
                    sidecar_input_hash: Some(suffix.repeat(4)),
                    sidecar_generation: Some(format!("{project_id}-{suffix}")),
                    projection_count: Some(1),
                    symbol_doc_count: Some(1),
                    dense_projection_count: Some(1),
                    semantic_policy_version: Some("graph_first_v1".into()),
                    graph_artifact_hash: Some("graph".into()),
                    dense_reason_counts_json: Some("{}".into()),
                    precise_semantic_import_status: None,
                    precise_semantic_import_reason: None,
                    precise_semantic_import_revision: None,
                    precise_semantic_import_producer: None,
                })
                .expect("upsert manifest");
        }

        let mut manifests = storage
            .list_retrieval_index_manifests()
            .expect("list manifests");
        manifests.sort_by(|left, right| left.project_id.cmp(&right.project_id));

        assert_eq!(
            manifests
                .iter()
                .map(|manifest| manifest.project_id.as_str())
                .collect::<Vec<_>>(),
            vec!["proj_a", "proj_b"]
        );
        assert_eq!(
            manifests[1].sidecar_generation.as_deref(),
            Some("proj_b-bbbbbbbbbbbbbbbb")
        );
    }

    #[test]
    fn list_retrieval_qdrant_collections_returns_distinct_names() {
        let dir = tempdir().expect("tempdir");
        let db_path = dir.path().join("codestory.db");
        let mut storage = Storage::open(&db_path).expect("open storage");
        for (project_id, collection) in [
            ("proj_a", "codestory_proj_a"),
            ("proj_b", "codestory_proj_b"),
            ("proj_c", "codestory_proj_a"),
        ] {
            storage
                .upsert_retrieval_index_manifest(&RetrievalIndexManifest {
                    project_id: project_id.into(),
                    lexical_version: "v1".into(),
                    qdrant_collection: collection.into(),
                    scip_revision: None,
                    built_at_epoch_ms: 1,
                    disk_bytes: None,
                    degraded_modes_json: "[]".into(),
                    embedding_backend: None,
                    embedding_dim: None,
                    sidecar_schema_version: None,
                    sidecar_input_hash: None,
                    sidecar_generation: None,
                    projection_count: None,
                    symbol_doc_count: None,
                    dense_projection_count: None,
                    semantic_policy_version: None,
                    graph_artifact_hash: None,
                    dense_reason_counts_json: None,
                    precise_semantic_import_status: None,
                    precise_semantic_import_reason: None,
                    precise_semantic_import_revision: None,
                    precise_semantic_import_producer: None,
                })
                .expect("upsert manifest");
        }
        let mut collections = storage
            .list_retrieval_qdrant_collections()
            .expect("list collections");
        collections.sort();
        assert_eq!(
            collections,
            vec![
                "codestory_proj_a".to_string(),
                "codestory_proj_b".to_string()
            ]
        );
    }
}
