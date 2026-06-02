use super::{Storage, StorageError};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetrievalIndexManifest {
    pub project_id: String,
    pub zoekt_version: String,
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
    /// Stable hash of all local inputs used to build Zoekt, Qdrant, and SCIP sidecars.
    pub sidecar_input_hash: Option<String>,
    /// Artifact generation id used for Zoekt/SCIP directories.
    pub sidecar_generation: Option<String>,
    /// Number of symbol projection rows included in the sidecar input hash.
    pub projection_count: Option<i64>,
}

impl Storage {
    pub fn upsert_retrieval_index_manifest(
        &mut self,
        manifest: &RetrievalIndexManifest,
    ) -> Result<(), StorageError> {
        self.conn.execute(
            "INSERT INTO retrieval_index_manifest (
                project_id,
                zoekt_version,
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
                projection_count
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
            ON CONFLICT(project_id) DO UPDATE SET
                zoekt_version = excluded.zoekt_version,
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
                projection_count = excluded.projection_count",
            rusqlite::params![
                manifest.project_id,
                manifest.zoekt_version,
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
            ],
        )?;
        Ok(())
    }

    pub fn get_retrieval_index_manifest(
        &self,
        project_id: &str,
    ) -> Result<Option<RetrievalIndexManifest>, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT
                project_id,
                zoekt_version,
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
                projection_count
             FROM retrieval_index_manifest
             WHERE project_id = ?1",
        )?;
        let mut rows = stmt.query(rusqlite::params![project_id])?;
        let Some(row) = rows.next()? else {
            return Ok(None);
        };
        Ok(Some(RetrievalIndexManifest {
            project_id: row.get(0)?,
            zoekt_version: row.get(1)?,
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
        }))
    }

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
                    zoekt_version: "v1".into(),
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
            zoekt_version: "v1".into(),
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
                    zoekt_version: "v1".into(),
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
