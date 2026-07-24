use crate::AppController;
use crate::support::node_display_name;
use codestory_contracts::api::{
    ApiError, BookmarkCategoryDto, BookmarkDto, CreateBookmarkCategoryRequest,
    CreateBookmarkRequest, NodeId, NodeKind, UpdateBookmarkCategoryRequest, UpdateBookmarkRequest,
};

fn parse_db_id(raw: &str, field_name: &str) -> Result<i64, ApiError> {
    raw.trim()
        .parse::<i64>()
        .map_err(|_| ApiError::invalid_argument(format!("Invalid {field_name}: {raw}")))
}

impl AppController {
    pub fn list_bookmark_categories(&self) -> Result<Vec<BookmarkCategoryDto>, ApiError> {
        let storage = self.open_storage_read_only()?;
        let categories = storage
            .get_bookmark_categories()
            .map_err(|e| ApiError::internal(format!("Failed to load bookmark categories: {e}")))?;
        Ok(categories
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

        let storage = self.open_storage()?;
        let id = storage
            .create_bookmark_category(name)
            .map_err(|e| ApiError::internal(format!("Failed to create bookmark category: {e}")))?;
        Ok(BookmarkCategoryDto {
            id: id.to_string(),
            name: name.to_string(),
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
        let storage = self.open_storage()?;
        let updated = storage
            .rename_bookmark_category(id, name)
            .map_err(|e| ApiError::internal(format!("Failed to update bookmark category: {e}")))?;
        if !updated {
            return Err(ApiError::not_found(format!(
                "Bookmark category not found: {id}"
            )));
        }
        Ok(BookmarkCategoryDto {
            id: id.to_string(),
            name: name.to_string(),
        })
    }

    pub fn delete_bookmark_category(&self, id: i64) -> Result<(), ApiError> {
        let storage = self.open_storage()?;
        storage
            .delete_bookmark_category(id)
            .map_err(|e| ApiError::internal(format!("Failed to delete bookmark category: {e}")))?;
        Ok(())
    }

    pub fn list_bookmarks(&self, category_id: Option<i64>) -> Result<Vec<BookmarkDto>, ApiError> {
        let storage = self.open_storage_read_only()?;
        let bookmarks = storage
            .get_bookmarks(category_id)
            .map_err(|e| ApiError::internal(format!("Failed to load bookmarks: {e}")))?;

        let mut response = Vec::with_capacity(bookmarks.len());
        for bookmark in bookmarks {
            let node = storage
                .get_node(bookmark.node_id)
                .map_err(|e| ApiError::internal(format!("Failed to load bookmark node: {e}")))?;
            let (node_label, node_kind, file_path) = match node {
                Some(node) => (
                    node_display_name(&node),
                    NodeKind::from(node.kind),
                    Self::file_path_for_node(&storage, &node)?,
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
            });
        }
        Ok(response)
    }

    pub fn create_bookmark(&self, req: CreateBookmarkRequest) -> Result<BookmarkDto, ApiError> {
        let node_id = req.node_id.to_core()?;
        let category_id = parse_db_id(&req.category_id, "category_id")?;
        let storage = self.open_storage()?;
        let node = storage
            .get_node(node_id)
            .map_err(|e| ApiError::internal(format!("Failed to load bookmark node: {e}")))?
            .ok_or_else(|| ApiError::not_found(format!("Node not found: {}", req.node_id.0)))?;
        let bookmark_id = storage
            .add_bookmark(category_id, node_id, req.comment.as_deref())
            .map_err(|e| ApiError::internal(format!("Failed to create bookmark: {e}")))?;

        Ok(BookmarkDto {
            id: bookmark_id.to_string(),
            category_id: category_id.to_string(),
            node_id: NodeId::from(node_id),
            comment: req.comment,
            node_label: node_display_name(&node),
            node_kind: NodeKind::from(node.kind),
            file_path: Self::file_path_for_node(&storage, &node)?,
        })
    }

    pub fn update_bookmark(
        &self,
        id: i64,
        req: UpdateBookmarkRequest,
    ) -> Result<BookmarkDto, ApiError> {
        let storage = self.open_storage()?;
        let category_id = req
            .category_id
            .as_deref()
            .map(|raw| parse_db_id(raw, "category_id"))
            .transpose()?;
        let comment_patch = req.comment.as_ref().map(|value| value.as_deref());
        storage
            .update_bookmark(id, category_id, comment_patch)
            .map_err(|e| ApiError::internal(format!("Failed to update bookmark: {e}")))?;
        let bookmark = storage
            .get_bookmarks(None)
            .map_err(|e| ApiError::internal(format!("Failed to reload bookmarks: {e}")))?
            .into_iter()
            .find(|bookmark| bookmark.id == id)
            .ok_or_else(|| ApiError::not_found(format!("Bookmark not found: {id}")))?;
        let node = storage
            .get_node(bookmark.node_id)
            .map_err(|e| ApiError::internal(format!("Failed to load bookmark node: {e}")))?;

        let (node_label, node_kind, file_path) = match node {
            Some(node) => (
                node_display_name(&node),
                NodeKind::from(node.kind),
                Self::file_path_for_node(&storage, &node)?,
            ),
            None => (bookmark.node_id.0.to_string(), NodeKind::UNKNOWN, None),
        };

        Ok(BookmarkDto {
            id: bookmark.id.to_string(),
            category_id: bookmark.category_id.to_string(),
            node_id: NodeId::from(bookmark.node_id),
            comment: bookmark.comment,
            node_label,
            node_kind,
            file_path,
        })
    }

    pub fn delete_bookmark(&self, id: i64) -> Result<(), ApiError> {
        let storage = self.open_storage()?;
        storage
            .delete_bookmark(id)
            .map_err(|e| ApiError::internal(format!("Failed to delete bookmark: {e}")))?;
        Ok(())
    }
}
