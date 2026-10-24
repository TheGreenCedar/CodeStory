use super::*;
use codestory_contracts::graph::{PublicExportAlias, PublicExportAliasStatus};

pub(super) const TABLES: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS public_export_alias (
        export_file_id INTEGER NOT NULL REFERENCES file(id) ON DELETE CASCADE,
        ordinal INTEGER NOT NULL,
        public_name TEXT NOT NULL,
        target_node_id INTEGER REFERENCES node(id) ON DELETE CASCADE,
        payload TEXT NOT NULL,
        PRIMARY KEY(export_file_id, ordinal)
    )",
    "CREATE INDEX IF NOT EXISTS idx_public_export_alias_name
        ON public_export_alias(lower(public_name), target_node_id)",
    "CREATE TABLE IF NOT EXISTS public_export_alias_publication (
        id INTEGER PRIMARY KEY CHECK(id=1),
        producer_version INTEGER NOT NULL,
        publication TEXT NOT NULL,
        complete INTEGER NOT NULL CHECK(complete IN (0,1)),
        row_count INTEGER NOT NULL CHECK(row_count>=0)
    )",
];

pub const PUBLIC_EXPORT_ALIAS_PRODUCER_VERSION: u32 = 2;

#[derive(Debug, Default)]
pub struct PublicExportAliasQuery {
    pub rows: Vec<PublicExportAlias>,
    pub ready: bool,
    pub complete: bool,
    pub bounded: bool,
}

impl Storage {
    /// Replaces the complete derived projection in the unpublished candidate.
    pub fn replace_public_export_aliases(
        &self,
        publication: &IndexPublicationRecord,
        aliases: &[PublicExportAlias],
        complete: bool,
    ) -> Result<(), StorageError> {
        for alias in aliases {
            if alias.public_name.is_empty()
                || alias.export_line == 0
                || self
                    .get_file_content_hash(alias.export_file_id.0)?
                    .as_deref()
                    != Some(alias.export_source_sha256.as_str())
            {
                return Err(StorageError::Other(
                    "public export source identity is invalid".into(),
                ));
            }
            if alias.status == PublicExportAliasStatus::Bound {
                let valid = match (
                    alias.target_node_id,
                    alias.target_file_id,
                    alias.target_source_sha256.as_deref(),
                ) {
                    (Some(target), Some(file), Some(hash)) => {
                        self.get_node(target)?.is_some_and(|node| {
                            node.file_node_id == Some(file)
                                && matches!(node.kind, NodeKind::CLASS | NodeKind::FUNCTION)
                        }) && self.get_file_content_hash(file.0)?.as_deref() == Some(hash)
                    }
                    _ => false,
                };
                if !valid {
                    return Err(StorageError::Other(
                        "public export target identity is invalid".into(),
                    ));
                }
            }
        }
        let transaction = self.conn.unchecked_transaction()?;
        transaction.execute("DELETE FROM public_export_alias", [])?;
        for (ordinal, alias) in aliases.iter().enumerate() {
            let payload = serde_json::to_string(alias)
                .map_err(|error| StorageError::Other(error.to_string()))?;
            transaction.execute(
                "INSERT INTO public_export_alias VALUES (?1,?2,?3,?4,?5)",
                params![
                    alias.export_file_id.0,
                    ordinal as i64,
                    alias.public_name,
                    alias.target_node_id.map(|id| id.0),
                    payload
                ],
            )?;
        }
        let publication = serde_json::to_string(publication)
            .map_err(|error| StorageError::Other(error.to_string()))?;
        transaction.execute(
            "INSERT OR REPLACE INTO public_export_alias_publication VALUES (1,?1,?2,?3,?4)",
            params![
                PUBLIC_EXPORT_ALIAS_PRODUCER_VERSION,
                publication,
                complete,
                aliases.len() as i64
            ],
        )?;
        transaction.commit()?;
        Ok(())
    }

    /// Reads named bindings only from the same complete core publication.
    pub fn public_export_aliases(
        &self,
        query: &str,
    ) -> Result<PublicExportAliasQuery, StorageError> {
        let mut result = PublicExportAliasQuery::default();
        let exists:bool=self.conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='public_export_alias_publication')",[],|row|row.get(0))?;
        if !exists {
            return Ok(result);
        }
        let manifest = self.conn.query_row(
            "SELECT producer_version,publication,complete,row_count FROM public_export_alias_publication WHERE id=1",
            [], |row| Ok((row.get::<_, u32>(0)?, row.get::<_, String>(1)?, row.get::<_, bool>(2)?,row.get::<_, i64>(3)?)),
        ).optional()?;
        let Some((version, publication, complete, row_count)) = manifest else {
            return Ok(result);
        };
        let publication: IndexPublicationRecord = serde_json::from_str(&publication)
            .map_err(|error| StorageError::Other(error.to_string()))?;
        if version != PUBLIC_EXPORT_ALIAS_PRODUCER_VERSION
            || self.get_index_publication()?.as_ref() != Some(&publication)
        {
            return Ok(result);
        }
        let actual_count: i64 =
            self.conn
                .query_row("SELECT count(*) FROM public_export_alias", [], |row| {
                    row.get(0)
                })?;
        if actual_count != row_count {
            return Ok(result);
        }
        result.ready = true;
        result.complete = complete;
        let mut stmt = self.conn.prepare(
            "SELECT payload FROM public_export_alias WHERE lower(public_name)=lower(?1)
             AND ordinal IN (SELECT min(ordinal) FROM public_export_alias
                 WHERE lower(public_name)=lower(?1) GROUP BY target_node_id
                 ORDER BY target_node_id LIMIT 202)
             ORDER BY target_node_id,export_file_id,ordinal",
        )?;
        let mut rows = stmt.query([query.trim()])?;
        let mut targets = HashSet::new();
        while let Some(row) = rows.next()? {
            let payload: String = row.get(0)?;
            let alias: PublicExportAlias = serde_json::from_str(&payload)
                .map_err(|error| StorageError::Other(error.to_string()))?;
            if alias.status != PublicExportAliasStatus::Bound {
                result.complete = false;
                continue;
            }
            let Some(id) = alias.target_node_id else {
                result.complete = false;
                continue;
            };
            let valid = self
                .get_file_content_hash(alias.export_file_id.0)?
                .as_deref()
                == Some(alias.export_source_sha256.as_str())
                && alias.target_file_id.is_some_and(|file| {
                    self.get_node(id).ok().flatten().is_some_and(|node| {
                        node.file_node_id == Some(file)
                            && matches!(node.kind, NodeKind::CLASS | NodeKind::FUNCTION)
                    })
                })
                && match (alias.target_file_id, alias.target_source_sha256.as_deref()) {
                    (Some(file), Some(hash)) => {
                        self.get_file_content_hash(file.0)?.as_deref() == Some(hash)
                    }
                    _ => false,
                };
            if !valid {
                result.complete = false;
                continue;
            }
            if !targets.insert(id) {
                continue;
            }
            if targets.len() > 200 {
                result.bounded = true;
                break;
            }
            result.rows.push(alias);
        }
        if targets.len() > 1 {
            result.complete = false;
        }
        Ok(result)
    }
}
