//! User annotations, owned by a versioned sidecar outside the promotion fence.
//!
//! Core projections are rebuilt and replaced wholesale by indexing, so user
//! state stored beside them is destroyed by ordinary editing. This sidecar
//! keeps annotations in their own database with their own schema version, and
//! anchors each bookmark by durable symbol evidence instead of a projection row
//! id. Resolution is recomputed against the live core on every read; the
//! sidecar never mirrors core rows, so there is exactly one source of truth for
//! an annotation at any instant.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

mod resolution;

#[cfg(test)]
mod tests;

pub use resolution::{
    AnnotationResolution, BookmarkAnchorEvidence, CoreAnchorCandidate, CoreAnchorIndex,
    OrphanReason, ResolutionStatus, resolve_bookmark,
};

/// Sidecar schema version, independent of the core database schema.
pub const ANNOTATION_SCHEMA_VERSION: u32 = 1;

/// Journal step recorded once the core annotation tables have been imported.
const CORE_IMPORT_JOURNAL_STEP: &str = "core-bookmark-import-v1";

const SIDECAR_BUSY_TIMEOUT: Duration = Duration::from_millis(2_500);

/// Typed annotation-store failures.
///
/// Every variant is fail-closed: an annotation operation that cannot prove it
/// is operating on the right sidecar refuses rather than writing.
#[derive(Debug, Error)]
pub enum AnnotationError {
    #[error("Annotation database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("Annotation sidecar io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(
        "Annotation sidecar {path} has schema version {found}, expected {ANNOTATION_SCHEMA_VERSION}"
    )]
    UnsupportedSchema { path: PathBuf, found: u32 },
    #[error(
        "Annotation sidecar {path} belongs to a different native root; export annotations from the original location and import them here"
    )]
    ForeignNativeRoot { path: PathBuf },
    #[error(
        "Annotation sidecar {path} cannot bind a project root without a native filesystem identity"
    )]
    UnidentifiedNativeRoot { path: PathBuf },
    #[error("Bookmark category not found: {0}")]
    CategoryNotFound(i64),
    #[error("Bookmark category name already exists: {0}")]
    DuplicateCategoryName(String),
    #[error("Bookmark not found: {0}")]
    BookmarkNotFound(String),
    #[error("Annotation migration failed: {0}")]
    Migration(String),
}

/// Native root binding observed by the caller for this open.
///
/// The identity token comes from a filesystem observation of the project root,
/// so a same-filesystem rename keeps it and a clone or cross-volume copy does
/// not. Callers that cannot observe the root pass `identity_token: None`, which
/// fails closed on a sidecar that is already bound.
#[derive(Debug, Clone)]
pub struct NativeRootBinding {
    pub identity_token: Option<String>,
    pub root_path: PathBuf,
}

impl NativeRootBinding {
    pub fn new(identity_token: Option<String>, root_path: impl Into<PathBuf>) -> Self {
        Self {
            identity_token,
            root_path: root_path.into(),
        }
    }
}

/// One annotation category.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnnotationCategory {
    pub id: i64,
    pub name: String,
}

/// One stored bookmark, with its durable anchor and last recorded resolution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnnotationBookmark {
    pub uuid: String,
    pub category_id: i64,
    pub canonical_id: Option<String>,
    pub file_identity: Option<String>,
    pub qualified_name: Option<String>,
    pub kind: Option<i64>,
    pub normalized_signature: Option<String>,
    pub start_line: Option<i64>,
    pub comment: Option<String>,
    pub resolution_status: ResolutionStatus,
    pub orphan_reason: Option<OrphanReason>,
    pub last_known_evidence: Option<BookmarkAnchorEvidence>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Anchor evidence supplied when a bookmark is created.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BookmarkAnchorInput {
    pub canonical_id: Option<String>,
    pub file_identity: Option<String>,
    pub qualified_name: Option<String>,
    pub kind: Option<i64>,
    pub normalized_signature: Option<String>,
    pub start_line: Option<i64>,
    pub comment: Option<String>,
    pub evidence: Option<BookmarkAnchorEvidence>,
}

/// Retained pre-migration export of the core annotation tables.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnnotationExport {
    pub schema_version: u32,
    pub exported_at_epoch_ms: i64,
    pub categories: Vec<AnnotationExportCategory>,
    pub bookmarks: Vec<AnnotationExportBookmark>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnnotationExportCategory {
    pub id: i64,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnnotationExportBookmark {
    pub category_id: i64,
    pub canonical_id: Option<String>,
    pub file_identity: Option<String>,
    pub qualified_name: Option<String>,
    pub kind: Option<i64>,
    pub normalized_signature: Option<String>,
    pub start_line: Option<i64>,
    pub comment: Option<String>,
}

/// Legacy core annotation rows handed to the cutover by the core store.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LegacyAnnotationSnapshot {
    pub categories: Vec<AnnotationCategory>,
    pub bookmarks: Vec<LegacyBookmarkRow>,
}

impl LegacyAnnotationSnapshot {
    pub fn is_empty(&self) -> bool {
        self.categories.is_empty() && self.bookmarks.is_empty()
    }
}

/// One legacy `bookmark_node` row already joined to its anchor evidence.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LegacyBookmarkRow {
    pub id: i64,
    pub category_id: i64,
    pub comment: Option<String>,
    pub canonical_id: Option<String>,
    pub file_identity: Option<String>,
    pub qualified_name: Option<String>,
    pub kind: Option<i64>,
    pub normalized_signature: Option<String>,
    pub start_line: Option<i64>,
}

const SCHEMA_STATEMENTS: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS annotation_schema_version (
        id INTEGER PRIMARY KEY CHECK (id = 1),
        version INTEGER NOT NULL CHECK (version > 0)
    )",
    "CREATE TABLE IF NOT EXISTS annotation_migration_journal (
        step TEXT PRIMARY KEY,
        source_fingerprint TEXT NOT NULL,
        completed_at_epoch_ms INTEGER NOT NULL CHECK (completed_at_epoch_ms >= 0)
    )",
    "CREATE TABLE IF NOT EXISTS native_root_location (
        id INTEGER PRIMARY KEY CHECK (id = 1),
        root_identity TEXT NOT NULL CHECK (length(root_identity) > 0),
        root_path TEXT NOT NULL,
        recorded_at_epoch_ms INTEGER NOT NULL CHECK (recorded_at_epoch_ms >= 0)
    )",
    "CREATE TABLE IF NOT EXISTS bookmark_category (
        id INTEGER PRIMARY KEY,
        name TEXT NOT NULL UNIQUE
    )",
    "CREATE TABLE IF NOT EXISTS bookmark (
        uuid TEXT PRIMARY KEY,
        category_id INTEGER NOT NULL REFERENCES bookmark_category(id) ON DELETE CASCADE,
        canonical_id TEXT,
        file_identity TEXT,
        qualified_name TEXT,
        kind INTEGER,
        normalized_signature TEXT,
        start_line INTEGER,
        comment TEXT,
        resolution_status TEXT NOT NULL CHECK (resolution_status IN ('bound', 'orphaned')),
        orphan_reason TEXT,
        last_known_evidence TEXT,
        created_at INTEGER NOT NULL CHECK (created_at >= 0),
        updated_at INTEGER NOT NULL CHECK (updated_at >= 0)
    )",
    "CREATE INDEX IF NOT EXISTS idx_bookmark_category ON bookmark(category_id)",
    "CREATE INDEX IF NOT EXISTS idx_bookmark_canonical_id ON bookmark(canonical_id)",
];

const BOOKMARK_COLUMNS: &str = "uuid, category_id, canonical_id, file_identity, qualified_name,
     kind, normalized_signature, start_line, comment, resolution_status,
     orphan_reason, last_known_evidence, created_at, updated_at";

/// Versioned annotation sidecar.
#[derive(Debug)]
pub struct AnnotationStore {
    conn: Connection,
    path: PathBuf,
}

impl AnnotationStore {
    /// Open the sidecar for writing, creating and migrating its schema.
    ///
    /// This is the mutating entry point: it materializes the sidecar file, so
    /// it belongs only to annotation writes and to operations that can replace
    /// core projections. Observational callers use [`Self::open_observational`].
    pub fn open_for_write(
        path: &Path,
        binding: &NativeRootBinding,
    ) -> Result<Self, AnnotationError> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent).map_err(|source| AnnotationError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let store = Self::connect(path)?;
        store.create_schema()?;
        store.bind_native_root(binding)?;
        Ok(store)
    }

    /// Open an existing sidecar without creating, migrating, or binding it.
    ///
    /// Returns `None` when no sidecar exists yet, so project-open, status, and
    /// doctor paths stay observational and never trigger the cutover.
    pub fn open_observational(path: &Path) -> Result<Option<Self>, AnnotationError> {
        if !path.is_file() {
            return Ok(None);
        }
        let store = Self::connect(path)?;
        let version = store.schema_version()?;
        if version != ANNOTATION_SCHEMA_VERSION {
            return Err(AnnotationError::UnsupportedSchema {
                path: path.to_path_buf(),
                found: version,
            });
        }
        Ok(Some(store))
    }

    fn connect(path: &Path) -> Result<Self, AnnotationError> {
        let conn = Connection::open(path)?;
        conn.busy_timeout(SIDECAR_BUSY_TIMEOUT)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        Ok(Self {
            conn,
            path: path.to_path_buf(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn create_schema(&self) -> Result<(), AnnotationError> {
        let stored = self.schema_version()?;
        if stored > ANNOTATION_SCHEMA_VERSION {
            return Err(AnnotationError::UnsupportedSchema {
                path: self.path.clone(),
                found: stored,
            });
        }
        for statement in SCHEMA_STATEMENTS {
            self.conn.execute(statement, [])?;
        }
        self.conn.execute(
            "INSERT INTO annotation_schema_version (id, version) VALUES (1, ?1)
             ON CONFLICT(id) DO UPDATE SET version = excluded.version",
            params![ANNOTATION_SCHEMA_VERSION],
        )?;
        Ok(())
    }

    /// Explicit schema-version row, `0` before the schema exists.
    pub fn schema_version(&self) -> Result<u32, AnnotationError> {
        let exists: Option<i64> = self
            .conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'annotation_schema_version'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        if exists.is_none() {
            return Ok(0);
        }
        let version: Option<i64> = self
            .conn
            .query_row(
                "SELECT version FROM annotation_schema_version WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .optional()?;
        Ok(version.unwrap_or(0).max(0) as u32)
    }

    /// Bind this sidecar to the caller's native project root.
    ///
    /// A first open records the binding. A later open with the same native
    /// identity refreshes the recorded path, which is what makes a
    /// same-filesystem move keep its annotations. Any other identity is a clone
    /// or a cross-volume copy and fails closed.
    fn bind_native_root(&self, binding: &NativeRootBinding) -> Result<(), AnnotationError> {
        let recorded: Option<String> = self
            .conn
            .query_row(
                "SELECT root_identity FROM native_root_location WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .optional()?;
        let Some(observed) = binding.identity_token.as_deref() else {
            return match recorded {
                None => Err(AnnotationError::UnidentifiedNativeRoot {
                    path: self.path.clone(),
                }),
                Some(_) => Err(AnnotationError::ForeignNativeRoot {
                    path: self.path.clone(),
                }),
            };
        };
        if recorded.as_deref().is_some_and(|stored| stored != observed) {
            return Err(AnnotationError::ForeignNativeRoot {
                path: self.path.clone(),
            });
        }
        self.conn.execute(
            "INSERT INTO native_root_location (id, root_identity, root_path, recorded_at_epoch_ms)
             VALUES (1, ?1, ?2, ?3)
             ON CONFLICT(id) DO UPDATE SET
                root_path = excluded.root_path,
                recorded_at_epoch_ms = excluded.recorded_at_epoch_ms",
            params![
                observed,
                binding.root_path.to_string_lossy().as_ref(),
                now_epoch_ms()
            ],
        )?;
        Ok(())
    }

    /// Recorded native root binding, if any.
    pub fn native_root_binding(&self) -> Result<Option<(String, PathBuf)>, AnnotationError> {
        Ok(self
            .conn
            .query_row(
                "SELECT root_identity, root_path FROM native_root_location WHERE id = 1",
                [],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?
            .map(|(identity, path)| (identity, PathBuf::from(path))))
    }

    /// Whether the core annotation import already completed.
    pub fn core_import_completed(&self) -> Result<bool, AnnotationError> {
        Ok(self
            .conn
            .query_row(
                "SELECT 1 FROM annotation_migration_journal WHERE step = ?1",
                params![CORE_IMPORT_JOURNAL_STEP],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .is_some())
    }

    /// Import the retained core annotation tables exactly once.
    ///
    /// The journal row and the imported rows commit in the same transaction, so
    /// a crash between them is impossible and a restart re-runs the import from
    /// the untouched legacy tables. The caller retains the backup export before
    /// calling; core rows are never deleted here, and after this step the
    /// sidecar is the only annotation writer.
    pub fn import_core_annotations(
        &mut self,
        snapshot: &LegacyAnnotationSnapshot,
        backup_path: &Path,
    ) -> Result<bool, AnnotationError> {
        if self.core_import_completed()? {
            return Ok(false);
        }
        write_backup_export(backup_path, snapshot)?;

        let fingerprint = format!(
            "categories={} bookmarks={}",
            snapshot.categories.len(),
            snapshot.bookmarks.len()
        );
        let now = now_epoch_ms();
        let tx = self.conn.transaction()?;
        let mut category_ids = BTreeMap::new();
        for category in &snapshot.categories {
            tx.execute(
                "INSERT INTO bookmark_category (id, name) VALUES (?1, ?2)
                 ON CONFLICT(name) DO NOTHING",
                params![category.id, category.name],
            )?;
            let resolved: i64 = tx.query_row(
                "SELECT id FROM bookmark_category WHERE name = ?1",
                params![category.name],
                |row| row.get(0),
            )?;
            category_ids.insert(category.id, resolved);
        }
        for bookmark in &snapshot.bookmarks {
            let Some(category_id) = category_ids.get(&bookmark.category_id).copied() else {
                // A legacy bookmark whose category row is gone has no owner to
                // restore it under; the retained backup keeps the evidence.
                continue;
            };
            let status = if bookmark.canonical_id.is_some()
                || (bookmark.file_identity.is_some() && bookmark.qualified_name.is_some())
            {
                ResolutionStatus::Bound
            } else {
                ResolutionStatus::Orphaned
            };
            let orphan_reason = (status == ResolutionStatus::Orphaned)
                .then_some(OrphanReason::TargetDeleted)
                .map(|reason| reason.as_str().to_string());
            tx.execute(
                &format!(
                    "INSERT INTO bookmark ({BOOKMARK_COLUMNS})
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)"
                ),
                params![
                    Uuid::new_v4().to_string(),
                    category_id,
                    bookmark.canonical_id,
                    bookmark.file_identity,
                    bookmark.qualified_name,
                    bookmark.kind,
                    bookmark.normalized_signature,
                    bookmark.start_line,
                    bookmark.comment,
                    status.as_str(),
                    orphan_reason,
                    None::<String>,
                    now,
                    now,
                ],
            )?;
        }
        tx.execute(
            "INSERT INTO annotation_migration_journal (step, source_fingerprint, completed_at_epoch_ms)
             VALUES (?1, ?2, ?3)",
            params![CORE_IMPORT_JOURNAL_STEP, fingerprint, now],
        )?;
        tx.commit()?;
        Ok(true)
    }

    pub fn create_category(&self, name: &str) -> Result<AnnotationCategory, AnnotationError> {
        let existing: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM bookmark_category WHERE name = ?1",
                params![name],
                |row| row.get(0),
            )
            .optional()?;
        if existing.is_some() {
            return Err(AnnotationError::DuplicateCategoryName(name.to_string()));
        }
        self.conn.execute(
            "INSERT INTO bookmark_category (name) VALUES (?1)",
            params![name],
        )?;
        Ok(AnnotationCategory {
            id: self.conn.last_insert_rowid(),
            name: name.to_string(),
        })
    }

    pub fn categories(&self) -> Result<Vec<AnnotationCategory>, AnnotationError> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, name FROM bookmark_category ORDER BY id ASC")?;
        let mut rows = stmt.query([])?;
        let mut categories = Vec::new();
        while let Some(row) = rows.next()? {
            categories.push(AnnotationCategory {
                id: row.get(0)?,
                name: row.get(1)?,
            });
        }
        Ok(categories)
    }

    pub fn rename_category(
        &self,
        id: i64,
        name: &str,
    ) -> Result<AnnotationCategory, AnnotationError> {
        let clashing: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM bookmark_category WHERE name = ?1 AND id <> ?2",
                params![name, id],
                |row| row.get(0),
            )
            .optional()?;
        if clashing.is_some() {
            return Err(AnnotationError::DuplicateCategoryName(name.to_string()));
        }
        let updated = self.conn.execute(
            "UPDATE bookmark_category SET name = ?1 WHERE id = ?2",
            params![name, id],
        )?;
        if updated == 0 {
            return Err(AnnotationError::CategoryNotFound(id));
        }
        Ok(AnnotationCategory {
            id,
            name: name.to_string(),
        })
    }

    /// Delete a category and, by declared cascade, the bookmarks it owns.
    pub fn delete_category(&self, id: i64) -> Result<(), AnnotationError> {
        let removed = self
            .conn
            .execute("DELETE FROM bookmark_category WHERE id = ?1", params![id])?;
        if removed == 0 {
            return Err(AnnotationError::CategoryNotFound(id));
        }
        Ok(())
    }

    pub fn create_bookmark(
        &self,
        category_id: i64,
        anchor: &BookmarkAnchorInput,
    ) -> Result<AnnotationBookmark, AnnotationError> {
        let category_exists: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM bookmark_category WHERE id = ?1",
                params![category_id],
                |row| row.get(0),
            )
            .optional()?;
        if category_exists.is_none() {
            return Err(AnnotationError::CategoryNotFound(category_id));
        }
        let now = now_epoch_ms();
        let bookmark = AnnotationBookmark {
            uuid: Uuid::new_v4().to_string(),
            category_id,
            canonical_id: anchor.canonical_id.clone(),
            file_identity: anchor.file_identity.clone(),
            qualified_name: anchor.qualified_name.clone(),
            kind: anchor.kind,
            normalized_signature: anchor.normalized_signature.clone(),
            start_line: anchor.start_line,
            comment: anchor.comment.clone(),
            resolution_status: ResolutionStatus::Bound,
            orphan_reason: None,
            last_known_evidence: anchor.evidence.clone(),
            created_at: now,
            updated_at: now,
        };
        self.insert_bookmark(&bookmark)?;
        Ok(bookmark)
    }

    fn insert_bookmark(&self, bookmark: &AnnotationBookmark) -> Result<(), AnnotationError> {
        self.conn.execute(
            &format!(
                "INSERT INTO bookmark ({BOOKMARK_COLUMNS})
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)"
            ),
            params![
                bookmark.uuid,
                bookmark.category_id,
                bookmark.canonical_id,
                bookmark.file_identity,
                bookmark.qualified_name,
                bookmark.kind,
                bookmark.normalized_signature,
                bookmark.start_line,
                bookmark.comment,
                bookmark.resolution_status.as_str(),
                bookmark.orphan_reason.map(|reason| reason.as_str()),
                encode_evidence(bookmark.last_known_evidence.as_ref())?,
                bookmark.created_at,
                bookmark.updated_at,
            ],
        )?;
        Ok(())
    }

    pub fn bookmarks(
        &self,
        category_id: Option<i64>,
    ) -> Result<Vec<AnnotationBookmark>, AnnotationError> {
        let query = format!(
            "SELECT {BOOKMARK_COLUMNS} FROM bookmark
             {} ORDER BY created_at ASC, uuid ASC",
            if category_id.is_some() {
                "WHERE category_id = ?1"
            } else {
                ""
            }
        );
        let mut stmt = self.conn.prepare(&query)?;
        let mut rows = match category_id {
            Some(id) => stmt.query(params![id])?,
            None => stmt.query([])?,
        };
        let mut bookmarks = Vec::new();
        while let Some(row) = rows.next()? {
            bookmarks.push(bookmark_from_row(row)?);
        }
        Ok(bookmarks)
    }

    pub fn bookmark(&self, uuid: &str) -> Result<Option<AnnotationBookmark>, AnnotationError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {BOOKMARK_COLUMNS} FROM bookmark WHERE uuid = ?1"
        ))?;
        let mut rows = stmt.query(params![uuid])?;
        match rows.next()? {
            Some(row) => Ok(Some(bookmark_from_row(row)?)),
            None => Ok(None),
        }
    }

    /// Patch the user-owned fields of one bookmark.
    ///
    /// `comment` follows the request's three-state patch: `None` leaves the
    /// stored comment untouched, `Some(None)` clears it.
    pub fn update_bookmark(
        &self,
        uuid: &str,
        category_id: Option<i64>,
        comment: Option<Option<&str>>,
    ) -> Result<AnnotationBookmark, AnnotationError> {
        let mut bookmark = self
            .bookmark(uuid)?
            .ok_or_else(|| AnnotationError::BookmarkNotFound(uuid.to_string()))?;
        if let Some(category_id) = category_id {
            let exists: Option<i64> = self
                .conn
                .query_row(
                    "SELECT id FROM bookmark_category WHERE id = ?1",
                    params![category_id],
                    |row| row.get(0),
                )
                .optional()?;
            if exists.is_none() {
                return Err(AnnotationError::CategoryNotFound(category_id));
            }
            bookmark.category_id = category_id;
        }
        match comment {
            Some(Some(value)) => bookmark.comment = Some(value.to_string()),
            Some(None) => bookmark.comment = None,
            None => {}
        }
        bookmark.updated_at = now_epoch_ms();
        self.conn.execute(
            "UPDATE bookmark SET category_id = ?1, comment = ?2, updated_at = ?3 WHERE uuid = ?4",
            params![
                bookmark.category_id,
                bookmark.comment,
                bookmark.updated_at,
                bookmark.uuid
            ],
        )?;
        Ok(bookmark)
    }

    pub fn delete_bookmark(&self, uuid: &str) -> Result<(), AnnotationError> {
        let removed = self
            .conn
            .execute("DELETE FROM bookmark WHERE uuid = ?1", params![uuid])?;
        if removed == 0 {
            return Err(AnnotationError::BookmarkNotFound(uuid.to_string()));
        }
        Ok(())
    }

    /// Persist one resolution outcome, rebinding the anchor when the pass
    /// produced better evidence.
    pub fn apply_resolution(
        &self,
        uuid: &str,
        resolution: &AnnotationResolution,
    ) -> Result<(), AnnotationError> {
        match resolution {
            AnnotationResolution::Bound { evidence, .. } => {
                self.conn.execute(
                    "UPDATE bookmark SET
                        canonical_id = ?1,
                        file_identity = ?2,
                        qualified_name = ?3,
                        kind = ?4,
                        normalized_signature = ?5,
                        start_line = ?6,
                        resolution_status = ?7,
                        orphan_reason = NULL,
                        last_known_evidence = ?8,
                        updated_at = ?9
                     WHERE uuid = ?10",
                    params![
                        evidence.canonical_id,
                        evidence.file_identity,
                        evidence.qualified_name,
                        evidence.kind,
                        evidence.normalized_signature,
                        evidence.start_line,
                        ResolutionStatus::Bound.as_str(),
                        encode_evidence(Some(evidence))?,
                        now_epoch_ms(),
                        uuid,
                    ],
                )?;
            }
            // The anchor and the last known evidence are user-owned state: an
            // orphan keeps both so an explicit relink has something to relink
            // from.
            AnnotationResolution::Orphaned { reason } => {
                self.conn.execute(
                    "UPDATE bookmark SET resolution_status = ?1, orphan_reason = ?2, updated_at = ?3
                     WHERE uuid = ?4",
                    params![
                        ResolutionStatus::Orphaned.as_str(),
                        reason.as_str(),
                        now_epoch_ms(),
                        uuid
                    ],
                )?;
            }
        }
        Ok(())
    }

    /// Export every annotation for the documented downgrade path.
    pub fn export(&self) -> Result<AnnotationExport, AnnotationError> {
        let categories = self
            .categories()?
            .into_iter()
            .map(|category| AnnotationExportCategory {
                id: category.id,
                name: category.name,
            })
            .collect();
        let bookmarks = self
            .bookmarks(None)?
            .into_iter()
            .map(|bookmark| AnnotationExportBookmark {
                category_id: bookmark.category_id,
                canonical_id: bookmark.canonical_id,
                file_identity: bookmark.file_identity,
                qualified_name: bookmark.qualified_name,
                kind: bookmark.kind,
                normalized_signature: bookmark.normalized_signature,
                start_line: bookmark.start_line,
                comment: bookmark.comment,
            })
            .collect();
        Ok(AnnotationExport {
            schema_version: ANNOTATION_SCHEMA_VERSION,
            exported_at_epoch_ms: now_epoch_ms(),
            categories,
            bookmarks,
        })
    }

    /// Import a previously exported annotation set into this sidecar.
    ///
    /// Categories merge by name; bookmarks are always added under new uuids, so
    /// importing into a populated sidecar never silently replaces user state.
    pub fn import(&mut self, export: &AnnotationExport) -> Result<usize, AnnotationError> {
        if export.schema_version != ANNOTATION_SCHEMA_VERSION {
            return Err(AnnotationError::Migration(format!(
                "annotation export has schema version {}, expected {ANNOTATION_SCHEMA_VERSION}",
                export.schema_version
            )));
        }
        let now = now_epoch_ms();
        let tx = self.conn.transaction()?;
        let mut category_ids = BTreeMap::new();
        for category in &export.categories {
            tx.execute(
                "INSERT INTO bookmark_category (name) VALUES (?1) ON CONFLICT(name) DO NOTHING",
                params![category.name],
            )?;
            let resolved: i64 = tx.query_row(
                "SELECT id FROM bookmark_category WHERE name = ?1",
                params![category.name],
                |row| row.get(0),
            )?;
            category_ids.insert(category.id, resolved);
        }
        let mut imported = 0usize;
        for bookmark in &export.bookmarks {
            let Some(category_id) = category_ids.get(&bookmark.category_id).copied() else {
                continue;
            };
            tx.execute(
                &format!(
                    "INSERT INTO bookmark ({BOOKMARK_COLUMNS})
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)"
                ),
                params![
                    Uuid::new_v4().to_string(),
                    category_id,
                    bookmark.canonical_id,
                    bookmark.file_identity,
                    bookmark.qualified_name,
                    bookmark.kind,
                    bookmark.normalized_signature,
                    bookmark.start_line,
                    bookmark.comment,
                    ResolutionStatus::Bound.as_str(),
                    None::<String>,
                    None::<String>,
                    now,
                    now,
                ],
            )?;
            imported += 1;
        }
        tx.commit()?;
        Ok(imported)
    }
}

fn bookmark_from_row(row: &rusqlite::Row<'_>) -> Result<AnnotationBookmark, AnnotationError> {
    let status: String = row.get(9)?;
    let orphan_reason: Option<String> = row.get(10)?;
    let evidence: Option<String> = row.get(11)?;
    Ok(AnnotationBookmark {
        uuid: row.get(0)?,
        category_id: row.get(1)?,
        canonical_id: row.get(2)?,
        file_identity: row.get(3)?,
        qualified_name: row.get(4)?,
        kind: row.get(5)?,
        normalized_signature: row.get(6)?,
        start_line: row.get(7)?,
        comment: row.get(8)?,
        resolution_status: ResolutionStatus::from_str(&status),
        orphan_reason: orphan_reason.as_deref().and_then(OrphanReason::from_str),
        last_known_evidence: evidence
            .as_deref()
            .and_then(|value| serde_json::from_str(value).ok()),
        created_at: row.get(12)?,
        updated_at: row.get(13)?,
    })
}

fn encode_evidence(
    evidence: Option<&BookmarkAnchorEvidence>,
) -> Result<Option<String>, AnnotationError> {
    evidence
        .map(|evidence| {
            serde_json::to_string(evidence).map_err(|error| {
                AnnotationError::Migration(format!("failed to encode anchor evidence: {error}"))
            })
        })
        .transpose()
}

fn write_backup_export(
    backup_path: &Path,
    snapshot: &LegacyAnnotationSnapshot,
) -> Result<(), AnnotationError> {
    if backup_path.exists() {
        return Ok(());
    }
    let export = AnnotationExport {
        schema_version: ANNOTATION_SCHEMA_VERSION,
        exported_at_epoch_ms: now_epoch_ms(),
        categories: snapshot
            .categories
            .iter()
            .map(|category| AnnotationExportCategory {
                id: category.id,
                name: category.name.clone(),
            })
            .collect(),
        bookmarks: snapshot
            .bookmarks
            .iter()
            .map(|bookmark| AnnotationExportBookmark {
                category_id: bookmark.category_id,
                canonical_id: bookmark.canonical_id.clone(),
                file_identity: bookmark.file_identity.clone(),
                qualified_name: bookmark.qualified_name.clone(),
                kind: bookmark.kind,
                normalized_signature: bookmark.normalized_signature.clone(),
                start_line: bookmark.start_line,
                comment: bookmark.comment.clone(),
            })
            .collect(),
    };
    let encoded = serde_json::to_vec_pretty(&export).map_err(|error| {
        AnnotationError::Migration(format!("failed to encode annotation backup: {error}"))
    })?;
    if let Some(parent) = backup_path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).map_err(|source| AnnotationError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    fs::write(backup_path, encoded).map_err(|source| AnnotationError::Io {
        path: backup_path.to_path_buf(),
        source,
    })
}

fn now_epoch_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or_default()
}
