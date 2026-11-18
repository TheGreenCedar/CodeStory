//! Pages of the core callable definition projection. Source bytes are owned by the reader.
use super::{Storage, StorageError};
use codestory_contracts::graph::{NodeId, NodeKind, OccurrenceKind};
use std::path::PathBuf;

#[derive(Debug)]
pub struct CallableDefinitionFile {
    pub id: i64,
    pub path: PathBuf,
    pub indexed: bool,
    pub complete: bool,
    pub content_hash: Option<String>,
}

#[derive(Debug)]
pub struct CallableDefinition {
    pub node_id: NodeId,
    pub name: String,
    pub start_line: u32,
    pub start_col: u32,
    pub end_line: u32,
    pub end_col: u32,
}

impl Storage {
    pub fn callable_definition_files_after(
        &self,
        after: Option<i64>,
    ) -> Result<Vec<CallableDefinitionFile>, StorageError> {
        const FIRST: &str =
            "SELECT id,path,indexed,complete,content_hash FROM file ORDER BY id LIMIT 512";
        const NEXT: &str = "SELECT id,path,indexed,complete,content_hash FROM file WHERE id>?1 ORDER BY id LIMIT 512";
        let mut statement = self
            .conn
            .prepare_cached(if after.is_some() { NEXT } else { FIRST })?;
        let file = |row: &rusqlite::Row<'_>| {
            Ok(CallableDefinitionFile {
                id: row.get(0)?,
                path: PathBuf::from(row.get::<_, String>(1)?),
                indexed: row.get(2)?,
                complete: row.get(3)?,
                content_hash: row.get(4)?,
            })
        };
        match after {
            Some(id) => statement
                .query_map([id], file)?
                .collect::<rusqlite::Result<_>>(),
            None => statement
                .query_map([], file)?
                .collect::<rusqlite::Result<_>>(),
        }
        .map_err(StorageError::from)
    }

    /// Same-file declaration occurrences owned by a physical source-file node.
    /// An indexed file with partial parser coverage may still own positive evidence.
    pub fn callable_definitions_for_file(
        &self,
        file_id: i64,
    ) -> Result<Vec<CallableDefinition>, StorageError> {
        self.read_callable_definitions(file_id, None)
    }

    pub fn callable_definitions_for_node(
        &self,
        file_id: i64,
        node_id: NodeId,
    ) -> Result<Vec<CallableDefinition>, StorageError> {
        self.read_callable_definitions(file_id, Some(node_id))
    }

    fn read_callable_definitions(
        &self,
        file_id: i64,
        node_id: Option<NodeId>,
    ) -> Result<Vec<CallableDefinition>, StorageError> {
        let selected_node = if node_id.is_some() {
            " AND o.element_id=?6"
        } else {
            ""
        };
        let sql = format!(
            "SELECT n.id, COALESCE(NULLIF(n.qualified_name,''), n.serialized_name),
                    o.start_line, o.start_col, o.end_line, o.end_col
             FROM occurrence o
             JOIN node n ON n.id=o.element_id AND n.file_node_id=o.file_node_id
             JOIN node owner ON owner.id=o.file_node_id AND owner.kind=?2
             WHERE o.file_node_id=?1 AND o.kind=?3 AND n.kind IN (?4, ?5){selected_node}
             ORDER BY o.start_line, o.end_line, o.start_col, o.end_col, n.id"
        );
        let mut statement = self.conn.prepare_cached(&sql)?;
        let mut parameters = vec![
            file_id,
            NodeKind::FILE as i64,
            OccurrenceKind::DEFINITION as i64,
            NodeKind::FUNCTION as i64,
            NodeKind::METHOD as i64,
        ];
        if let Some(id) = node_id {
            parameters.push(id.0);
        }
        statement
            .query_map(rusqlite::params_from_iter(parameters), |row| {
                Ok(CallableDefinition {
                    node_id: NodeId(row.get(0)?),
                    name: row.get(1)?,
                    start_line: row.get(2)?,
                    start_col: row.get(3)?,
                    end_line: row.get(4)?,
                    end_col: row.get(5)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()
            .map_err(StorageError::from)
    }
}
