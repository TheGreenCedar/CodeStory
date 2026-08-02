//! Runtime annotation CRUD, owned by the versioned sidecar.
//!
//! Reads never materialize or migrate the sidecar, so project-open, status, and
//! doctor stay observational and a read-only session cannot start the cutover.
//! Writes cut over first and then write only to the sidecar: before the cutover
//! the retained core tables are the source of truth, after it the sidecar is,
//! and there is no instant at which both are.

use crate::AppController;
use crate::support::node_display_name;
use codestory_contracts::api::{
    ApiError, BookmarkCategoryDto, BookmarkDto, BookmarkEvidenceDto, BookmarkOrphanReasonDto,
    BookmarkResolutionStatusDto, CreateBookmarkCategoryRequest, CreateBookmarkRequest, NodeId,
    NodeKind, UpdateBookmarkCategoryRequest, UpdateBookmarkRequest,
};
use codestory_contracts::owned_artifacts;
use codestory_store::{
    AnnotationBookmark, AnnotationError, AnnotationResolution, AnnotationStore,
    BookmarkAnchorInput, CoreAnchorCandidate, CoreAnchorIndex, NativeRootBinding, OrphanReason,
    ResolutionStatus, Store, resolve_bookmark,
};

/// Proof that user annotations already moved out of the core database.
///
/// The private field is the point: only
/// [`AppController::ensure_annotations_owned_before_core_replacement`] can
/// mint one, and every function that replaces core projections takes one by
/// reference. A future entry point that forgets the cutover therefore does not
/// compile, instead of silently publishing a from-scratch database over the
/// retained legacy annotation tables.
#[derive(Debug)]
pub(crate) struct AnnotationsOwned(());

impl AnnotationsOwned {
    /// Mint the proof for a test that drives indexing without a controller.
    #[cfg(test)]
    pub(crate) fn assume_owned_for_test() -> Self {
        Self(())
    }
}

fn parse_db_id(raw: &str, field_name: &str) -> Result<i64, ApiError> {
    raw.trim()
        .parse::<i64>()
        .map_err(|_| ApiError::invalid_argument(format!("Invalid {field_name}: {raw}")))
}

fn annotation_error(context: &str, error: AnnotationError) -> ApiError {
    match error {
        AnnotationError::BookmarkNotFound(id) => {
            ApiError::not_found(format!("Bookmark not found: {id}"))
        }
        AnnotationError::CategoryNotFound(id) => {
            ApiError::not_found(format!("Bookmark category not found: {id}"))
        }
        AnnotationError::DuplicateCategoryName(name) => {
            ApiError::invalid_argument(format!("Bookmark category already exists: {name}"))
        }
        other => ApiError::internal(format!("{context}: {other}")),
    }
}

/// Selective core lookups backing one resolution pass.
struct CoreAnchors<'a> {
    storage: &'a Store,
    generation: Option<i64>,
}

impl CoreAnchorIndex for CoreAnchors<'_> {
    fn current_generation(&self) -> Option<i64> {
        self.generation
    }

    fn candidates_by_canonical_id(&self, canonical_id: &str) -> Vec<CoreAnchorCandidate> {
        self.storage
            .annotation_anchors_by_canonical_id(canonical_id)
            .unwrap_or_default()
    }

    fn candidates_by_anchor_tuple(
        &self,
        file_identity: &str,
        qualified_name: &str,
        kind: i64,
    ) -> Vec<CoreAnchorCandidate> {
        self.storage
            .annotation_anchors_by_anchor_tuple(file_identity, qualified_name, kind)
            .unwrap_or_default()
    }

    fn candidates_by_qualified_name(
        &self,
        qualified_name: &str,
        kind: i64,
    ) -> Vec<CoreAnchorCandidate> {
        self.storage
            .annotation_anchors_by_qualified_name(qualified_name, kind)
            .unwrap_or_default()
    }

    fn candidates_by_normalized_signature(
        &self,
        normalized_signature: &str,
        file_identity: &str,
        kind: i64,
    ) -> Vec<CoreAnchorCandidate> {
        self.storage
            .annotation_anchors_by_normalized_signature(normalized_signature, file_identity, kind)
            .unwrap_or_default()
    }
}

impl AppController {
    pub(crate) fn annotations_sidecar_path(&self) -> Result<std::path::PathBuf, ApiError> {
        Ok(owned_artifacts::annotations_sidecar_path(
            &self.require_storage_path()?,
        ))
    }

    fn native_root_binding(&self) -> Result<NativeRootBinding, ApiError> {
        let root = self.require_project_root()?;
        let identity_token =
            codestory_workspace::workspace_path_identity_token(&root).map_err(|error| {
                ApiError::internal(format!(
                    "Failed to observe the project root's native identity: {error}"
                ))
            })?;
        Ok(NativeRootBinding::new(identity_token, root))
    }

    /// Open the sidecar for writing, running the one-time cutover if needed.
    ///
    /// This is the mutating trigger: an annotation write, or an operation that
    /// can replace core projections, is the only thing that may create the
    /// sidecar or import the retained core tables.
    pub(crate) fn open_annotations_for_write(&self) -> Result<AnnotationStore, ApiError> {
        let sidecar_path = self.annotations_sidecar_path()?;
        let binding = self.native_root_binding()?;
        let mut annotations = AnnotationStore::open_for_write(&sidecar_path, &binding)
            .map_err(|error| annotation_error("Failed to open the annotations sidecar", error))?;
        if annotations
            .core_import_completed()
            .map_err(|error| annotation_error("Failed to read the annotation journal", error))?
        {
            return Ok(annotations);
        }

        // Opening the core store for write applies the schema-31 migration,
        // which is the writer barrier that stops an older CLI from forking
        // annotation truth into the retained legacy tables. The import reads
        // those tables and never deletes them, so a crash before the journal
        // commits simply re-runs from the same source.
        let storage = self.open_storage()?;
        let snapshot = storage.legacy_annotation_snapshot().map_err(|error| {
            ApiError::internal(format!("Failed to read legacy annotations: {error}"))
        })?;
        let backup_path =
            owned_artifacts::annotations_migration_backup_path(&self.require_storage_path()?);
        annotations
            .import_core_annotations(&snapshot, &backup_path)
            .map_err(|error| annotation_error("Failed to migrate annotations", error))?;
        Ok(annotations)
    }

    /// Open the sidecar without creating or migrating it.
    fn open_annotations_observational(&self) -> Result<Option<AnnotationStore>, ApiError> {
        let sidecar_path = self.annotations_sidecar_path()?;
        AnnotationStore::open_observational(&sidecar_path)
            .map_err(|error| annotation_error("Failed to open the annotations sidecar", error))
    }

    /// Open the sidecar for reading, or `None` while core still owns
    /// annotations.
    ///
    /// Ownership switches when the import *commits*, not when the file
    /// appears. `open_annotations_for_write` creates and binds the sidecar
    /// before it imports, so any failure in between — a backup write onto a
    /// full disk, an unreadable core, a process kill — leaves an empty sidecar
    /// sitting on top of intact legacy rows. Switching reads on file existence
    /// would report zero annotations for a user whose annotations are all
    /// still there. The journal row is the only durable statement that the
    /// sidecar has become the source of truth, so it is what reads switch on.
    fn open_annotations_for_read(&self) -> Result<Option<AnnotationStore>, ApiError> {
        let Some(annotations) = self.open_annotations_observational()? else {
            return Ok(None);
        };
        let imported = annotations
            .core_import_completed()
            .map_err(|error| annotation_error("Failed to read the annotation journal", error))?;
        Ok(imported.then_some(annotations))
    }

    /// Move annotations into the sidecar before core projections are replaced.
    ///
    /// A full refresh installs a freshly built database that never carried the
    /// legacy annotation tables, so the cutover has to happen *before* the
    /// operation that replaces core, not after it. A project with no
    /// annotations gets no sidecar: publication is not a reason to create one.
    ///
    /// The returned [`AnnotationsOwned`] is the proof every core-replacing
    /// entry point demands, which is what makes the ordering structural rather
    /// than a convention two adjacent call sites happen to follow.
    pub(crate) fn ensure_annotations_owned_before_core_replacement(
        &self,
    ) -> Result<AnnotationsOwned, ApiError> {
        if !self.annotations_sidecar_path()?.is_file() && !self.core_owns_legacy_annotations()? {
            return Ok(AnnotationsOwned(()));
        }
        self.open_annotations_for_write()
            .map(|_| AnnotationsOwned(()))
    }

    fn core_owns_legacy_annotations(&self) -> Result<bool, ApiError> {
        let storage_path = self.require_storage_path()?;
        if !storage_path.is_file() {
            return Ok(false);
        }
        Store::database_legacy_annotation_count(&storage_path)
            .map(|count| count > 0)
            .map_err(|error| {
                ApiError::internal(format!("Failed to inspect legacy annotations: {error}"))
            })
    }

    /// Re-resolve every annotation and persist the outcome.
    ///
    /// Runs after an operation that replaced core projections so anchor
    /// evidence advances one generation at a time, which is what keeps a
    /// later rename or move inside the adjacent-generation rebind gate.
    pub(crate) fn rebind_annotations_after_core_publication(&self) -> Result<(), ApiError> {
        // An absent sidecar means this project has no annotations to rebind,
        // and publication is not a reason to create one.
        if !self.annotations_sidecar_path()?.is_file() {
            return Ok(());
        }
        let annotations = self.open_annotations_for_write()?;
        let storage = self.open_storage_read_only()?;
        let anchors = CoreAnchors {
            storage: &storage,
            generation: Self::core_generation(&storage),
        };
        let bookmarks = annotations
            .bookmarks(None)
            .map_err(|error| annotation_error("Failed to load annotations", error))?;
        for bookmark in bookmarks {
            let resolution = resolve_bookmark(&bookmark, &anchors);
            annotations
                .apply_resolution(&bookmark.uuid, &resolution)
                .map_err(|error| annotation_error("Failed to record annotation binding", error))?;
        }
        Ok(())
    }

    fn core_generation(storage: &Store) -> Option<i64> {
        storage
            .get_complete_index_publication()
            .ok()
            .flatten()
            .and_then(|publication| i64::try_from(publication.generation).ok())
    }

    fn bookmark_dto(
        storage: Option<&Store>,
        bookmark: &AnnotationBookmark,
        resolution: &AnnotationResolution,
    ) -> Result<BookmarkDto, ApiError> {
        let node = match (storage, resolution.node_id()) {
            (Some(storage), Some(node_id)) => storage
                .get_node(codestory_contracts::graph::NodeId(node_id))
                .map_err(|e| ApiError::internal(format!("Failed to load bookmark node: {e}")))?,
            _ => None,
        };
        let (node_id, node_label, node_kind, file_path) = match node {
            Some(node) => (
                NodeId::from(node.id),
                node_display_name(&node),
                NodeKind::from(node.kind),
                storage
                    .map(|storage| Self::file_path_for_node(storage, &node))
                    .transpose()?
                    .flatten(),
            ),
            None => (
                NodeId(
                    bookmark
                        .last_known_evidence
                        .as_ref()
                        .and_then(|evidence| evidence.node_id)
                        .unwrap_or_default()
                        .to_string(),
                ),
                bookmark
                    .qualified_name
                    .clone()
                    .or_else(|| bookmark.canonical_id.clone())
                    .unwrap_or_else(|| bookmark.uuid.clone()),
                NodeKind::UNKNOWN,
                bookmark.file_identity.clone(),
            ),
        };
        Ok(BookmarkDto {
            id: bookmark.uuid.clone(),
            category_id: bookmark.category_id.to_string(),
            node_id,
            comment: bookmark.comment.clone(),
            node_label,
            node_kind,
            file_path,
            resolution_status: match resolution.status() {
                ResolutionStatus::Bound => BookmarkResolutionStatusDto::Bound,
                ResolutionStatus::Orphaned => BookmarkResolutionStatusDto::Orphaned,
            },
            orphan_reason: resolution.orphan_reason().map(orphan_reason_dto),
            last_known_evidence: bookmark.last_known_evidence.as_ref().map(|evidence| {
                BookmarkEvidenceDto {
                    generation: evidence.generation,
                    file_path: evidence.file_identity.clone(),
                    qualified_name: evidence.qualified_name.clone(),
                    start_line: evidence.start_line,
                }
            }),
        })
    }

    pub fn list_bookmark_categories(&self) -> Result<Vec<BookmarkCategoryDto>, ApiError> {
        let Some(annotations) = self.open_annotations_for_read()? else {
            return self.legacy_bookmark_categories();
        };
        Ok(annotations
            .categories()
            .map_err(|error| annotation_error("Failed to load bookmark categories", error))?
            .into_iter()
            .map(|category| BookmarkCategoryDto {
                id: category.id.to_string(),
                name: category.name,
            })
            .collect())
    }

    /// Read the retained core tables while the sidecar does not exist yet.
    fn legacy_bookmark_categories(&self) -> Result<Vec<BookmarkCategoryDto>, ApiError> {
        let storage = self.open_storage_read_only()?;
        Ok(storage
            .get_bookmark_categories()
            .map_err(|e| ApiError::internal(format!("Failed to load bookmark categories: {e}")))?
            .into_iter()
            .map(|category| BookmarkCategoryDto {
                id: category.id.to_string(),
                name: category.name,
            })
            .collect())
    }

    pub fn create_bookmark_category(
        &self,
        req: CreateBookmarkCategoryRequest,
    ) -> Result<BookmarkCategoryDto, ApiError> {
        let name = req.name.trim();
        if name.is_empty() {
            return Err(ApiError::invalid_argument(
                "Bookmark category name cannot be empty.",
            ));
        }

        let annotations = self.open_annotations_for_write()?;
        let category = annotations
            .create_category(name)
            .map_err(|error| annotation_error("Failed to create bookmark category", error))?;
        Ok(BookmarkCategoryDto {
            id: category.id.to_string(),
            name: category.name,
        })
    }

    pub fn update_bookmark_category(
        &self,
        id: i64,
        req: UpdateBookmarkCategoryRequest,
    ) -> Result<BookmarkCategoryDto, ApiError> {
        let name = req.name.trim();
        if name.is_empty() {
            return Err(ApiError::invalid_argument(
                "Bookmark category name cannot be empty.",
            ));
        }
        let annotations = self.open_annotations_for_write()?;
        let category = annotations
            .rename_category(id, name)
            .map_err(|error| annotation_error("Failed to update bookmark category", error))?;
        Ok(BookmarkCategoryDto {
            id: category.id.to_string(),
            name: category.name,
        })
    }

    pub fn delete_bookmark_category(&self, id: i64) -> Result<(), ApiError> {
        let annotations = self.open_annotations_for_write()?;
        annotations
            .delete_category(id)
            .map_err(|error| annotation_error("Failed to delete bookmark category", error))
    }

    pub fn list_bookmarks(&self, category_id: Option<i64>) -> Result<Vec<BookmarkDto>, ApiError> {
        let Some(annotations) = self.open_annotations_for_read()? else {
            return self.legacy_bookmarks(category_id);
        };
        // Annotations outlive the core they point at. When core is absent or
        // unreadable — a quarantined derived cache, a reset, a schema the
        // runtime will not read — the sidecar still reports every annotation
        // from its own durable state instead of failing the read.
        let storage = self.open_storage_read_only().ok();
        let anchors = storage.as_deref().map(|storage| CoreAnchors {
            storage,
            generation: Self::core_generation(storage),
        });
        let bookmarks = annotations
            .bookmarks(category_id)
            .map_err(|error| annotation_error("Failed to load bookmarks", error))?;
        let mut response = Vec::with_capacity(bookmarks.len());
        for bookmark in bookmarks {
            // A read resolves live but never writes: an observational caller
            // must not be able to mutate the sidecar.
            let resolution = match anchors.as_ref() {
                Some(anchors) => resolve_bookmark(&bookmark, anchors),
                None => stored_resolution(&bookmark),
            };
            response.push(Self::bookmark_dto(
                storage.as_deref(),
                &bookmark,
                &resolution,
            )?);
        }
        Ok(response)
    }

    /// Read the retained core tables while the sidecar does not exist yet.
    fn legacy_bookmarks(&self, category_id: Option<i64>) -> Result<Vec<BookmarkDto>, ApiError> {
        let storage = self.open_storage_read_only()?;
        let bookmarks = storage
            .get_bookmarks(category_id)
            .map_err(|e| ApiError::internal(format!("Failed to load bookmarks: {e}")))?;
        let mut response = Vec::with_capacity(bookmarks.len());
        for bookmark in bookmarks {
            let node = storage
                .get_node(bookmark.node_id)
                .map_err(|e| ApiError::internal(format!("Failed to load bookmark node: {e}")))?;
            let (node_label, node_kind, file_path) = match node.as_ref() {
                Some(node) => (
                    node_display_name(node),
                    NodeKind::from(node.kind),
                    Self::file_path_for_node(&storage, node)?,
                ),
                None => (bookmark.node_id.0.to_string(), NodeKind::UNKNOWN, None),
            };
            response.push(BookmarkDto {
                id: bookmark.id.to_string(),
                category_id: bookmark.category_id.to_string(),
                node_id: NodeId::from(bookmark.node_id),
                comment: bookmark.comment,
                node_label,
                node_kind,
                file_path,
                resolution_status: if node.is_some() {
                    BookmarkResolutionStatusDto::Bound
                } else {
                    BookmarkResolutionStatusDto::Orphaned
                },
                orphan_reason: node
                    .is_none()
                    .then_some(BookmarkOrphanReasonDto::TargetDeleted),
                last_known_evidence: None,
            });
        }
        Ok(response)
    }

    pub fn create_bookmark(&self, req: CreateBookmarkRequest) -> Result<BookmarkDto, ApiError> {
        let node_id = req.node_id.to_core()?;
        let category_id = parse_db_id(&req.category_id, "category_id")?;
        let annotations = self.open_annotations_for_write()?;
        let storage = self.open_storage()?;
        let node = storage
            .get_node(node_id)
            .map_err(|e| ApiError::internal(format!("Failed to load bookmark node: {e}")))?
            .ok_or_else(|| ApiError::not_found(format!("Node not found: {}", req.node_id.0)))?;
        let anchor = storage
            .annotation_anchor_for_node(node_id)
            .map_err(|e| ApiError::internal(format!("Failed to read bookmark anchor: {e}")))?
            .ok_or_else(|| ApiError::not_found(format!("Node not found: {}", req.node_id.0)))?;
        let anchors = CoreAnchors {
            storage: &storage,
            generation: Self::core_generation(&storage),
        };
        // The same evidence the rebind pass records, including how well this
        // anchor separated its symbol from its neighbours: a later rename or
        // move may only be inferred from evidence that was discriminating when
        // it was proven.
        let evidence = codestory_store::anchor_evidence(&anchor, anchors.generation, &anchors);
        let bookmark = annotations
            .create_bookmark(
                category_id,
                &BookmarkAnchorInput {
                    canonical_id: anchor.canonical_id,
                    file_identity: anchor.file_identity,
                    qualified_name: anchor.qualified_name,
                    kind: anchor.kind,
                    normalized_signature: anchor.normalized_signature,
                    start_line: anchor.start_line,
                    comment: req.comment.clone(),
                    evidence: Some(evidence),
                },
            )
            .map_err(|error| annotation_error("Failed to create bookmark", error))?;

        Ok(BookmarkDto {
            id: bookmark.uuid,
            category_id: category_id.to_string(),
            node_id: NodeId::from(node_id),
            comment: req.comment,
            node_label: node_display_name(&node),
            node_kind: NodeKind::from(node.kind),
            file_path: Self::file_path_for_node(&storage, &node)?,
            resolution_status: BookmarkResolutionStatusDto::Bound,
            orphan_reason: None,
            last_known_evidence: bookmark
                .last_known_evidence
                .map(|evidence| BookmarkEvidenceDto {
                    generation: evidence.generation,
                    file_path: evidence.file_identity,
                    qualified_name: evidence.qualified_name,
                    start_line: evidence.start_line,
                }),
        })
    }

    /// Address a bookmark by the id the API last handed out.
    ///
    /// A pre-cutover read returns retained legacy row ids, and the next call
    /// on one of those ids is often the write that performs the cutover. The
    /// import derives each imported uuid from its legacy row id, so a legacy
    /// id keeps addressing the same annotation across that boundary. Sidecar
    /// uuids are unaffected: they are only translated when they are not
    /// present and they parse as a legacy row id.
    fn addressed_bookmark_id(annotations: &AnnotationStore, id: &str) -> Result<String, ApiError> {
        let known = annotations
            .bookmark(id)
            .map_err(|error| annotation_error("Failed to load bookmark", error))?
            .is_some();
        if known {
            return Ok(id.to_string());
        }
        match id.trim().parse::<i64>() {
            Ok(legacy_id) => Ok(codestory_store::legacy_bookmark_uuid(legacy_id)),
            Err(_) => Ok(id.to_string()),
        }
    }

    pub fn update_bookmark(
        &self,
        id: &str,
        req: UpdateBookmarkRequest,
    ) -> Result<BookmarkDto, ApiError> {
        let annotations = self.open_annotations_for_write()?;
        let id = &Self::addressed_bookmark_id(&annotations, id)?;
        let category_id = req
            .category_id
            .as_deref()
            .map(|raw| parse_db_id(raw, "category_id"))
            .transpose()?;
        let comment_patch = req.comment.as_ref().map(|value| value.as_deref());
        let bookmark = annotations
            .update_bookmark(id, category_id, comment_patch)
            .map_err(|error| annotation_error("Failed to update bookmark", error))?;
        let storage = self.open_storage_read_only()?;
        let anchors = CoreAnchors {
            storage: &storage,
            generation: Self::core_generation(&storage),
        };
        let resolution = resolve_bookmark(&bookmark, &anchors);
        annotations
            .apply_resolution(&bookmark.uuid, &resolution)
            .map_err(|error| annotation_error("Failed to record annotation binding", error))?;
        Self::bookmark_dto(Some(&storage), &bookmark, &resolution)
    }

    pub fn delete_bookmark(&self, id: &str) -> Result<(), ApiError> {
        let annotations = self.open_annotations_for_write()?;
        let id = Self::addressed_bookmark_id(&annotations, id)?;
        annotations
            .delete_bookmark(&id)
            .map_err(|error| annotation_error("Failed to delete bookmark", error))
    }

    /// Export every annotation for the documented downgrade path.
    pub fn export_annotations(&self) -> Result<codestory_store::AnnotationExport, ApiError> {
        let annotations = self.open_annotations_for_write()?;
        annotations
            .export()
            .map_err(|error| annotation_error("Failed to export annotations", error))
    }

    /// Import a previously exported annotation set, for a clone or a
    /// cross-volume copy that cannot inherit a native root binding.
    pub fn import_annotations(
        &self,
        export: &codestory_store::AnnotationExport,
    ) -> Result<usize, ApiError> {
        let mut annotations = self.open_annotations_for_write()?;
        annotations
            .import(export)
            .map_err(|error| annotation_error("Failed to import annotations", error))
    }
}

/// Last recorded resolution, used when the live core cannot be consulted.
fn stored_resolution(bookmark: &AnnotationBookmark) -> AnnotationResolution {
    match (
        bookmark.resolution_status,
        bookmark.last_known_evidence.as_ref(),
    ) {
        (ResolutionStatus::Bound, Some(evidence)) => AnnotationResolution::Bound {
            node_id: evidence.node_id.unwrap_or_default(),
            evidence: evidence.clone(),
        },
        _ => AnnotationResolution::Orphaned {
            reason: bookmark
                .orphan_reason
                .unwrap_or(OrphanReason::UnresolvableAnchor),
        },
    }
}

fn orphan_reason_dto(reason: OrphanReason) -> BookmarkOrphanReasonDto {
    match reason {
        OrphanReason::TargetDeleted => BookmarkOrphanReasonDto::TargetDeleted,
        OrphanReason::AmbiguousMatch => BookmarkOrphanReasonDto::AmbiguousMatch,
        OrphanReason::GenerationGap => BookmarkOrphanReasonDto::GenerationGap,
        OrphanReason::SignatureChanged => BookmarkOrphanReasonDto::SignatureChanged,
        OrphanReason::UnresolvableAnchor => BookmarkOrphanReasonDto::UnresolvableAnchor,
    }
}
