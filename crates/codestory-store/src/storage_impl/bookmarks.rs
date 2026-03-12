use super::*;

pub(super) fn create_bookmark_category(conn: &Connection, name: &str) -> Result<i64, StorageError> {
    conn.execute(
        "INSERT INTO bookmark_category (name) VALUES (?1)",
        params![name],
    )?;
    Ok(conn.last_insert_rowid())
}

pub(super) fn get_bookmark_categories(
    conn: &Connection,
) -> Result<Vec<BookmarkCategory>, StorageError> {
    let mut stmt = conn.prepare("SELECT id, name FROM bookmark_category")?;
    let mut categories = Vec::new();
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        categories.push(BookmarkCategory {
            id: row.get(0)?,
            name: row.get(1)?,
        });
    }
    Ok(categories)
}

pub(super) fn delete_bookmark_category(conn: &Connection, id: i64) -> Result<(), StorageError> {
    conn.execute(
        "DELETE FROM bookmark_node WHERE category_id = ?1",
        params![id],
    )?;
    conn.execute("DELETE FROM bookmark_category WHERE id = ?1", params![id])?;
    Ok(())
}

pub(super) fn rename_bookmark_category(
    conn: &Connection,
    id: i64,
    new_name: &str,
) -> Result<bool, StorageError> {
    let updated = conn.execute(
        "UPDATE bookmark_category SET name = ?1 WHERE id = ?2",
        params![new_name, id],
    )?;
    Ok(updated > 0)
}

pub(super) fn add_bookmark(
    conn: &Connection,
    category_id: i64,
    node_id: NodeId,
    comment: Option<&str>,
) -> Result<i64, StorageError> {
    conn.execute(
        "INSERT INTO bookmark_node (category_id, node_id, comment) VALUES (?1, ?2, ?3)",
        params![category_id, node_id.0, comment],
    )?;
    Ok(conn.last_insert_rowid())
}

pub(super) fn get_bookmarks(
    conn: &Connection,
    category_id: Option<i64>,
) -> Result<Vec<Bookmark>, StorageError> {
    let query = match category_id {
        Some(_) => {
            "SELECT id, category_id, node_id, comment FROM bookmark_node WHERE category_id = ?1"
        }
        None => "SELECT id, category_id, node_id, comment FROM bookmark_node",
    };
    let mut stmt = conn.prepare(query)?;
    let mut bookmarks = Vec::new();

    let mut rows = if let Some(cat_id) = category_id {
        stmt.query(params![cat_id])?
    } else {
        stmt.query([])?
    };

    while let Some(row) = rows.next()? {
        bookmarks.push(Bookmark {
            id: row.get(0)?,
            category_id: row.get(1)?,
            node_id: NodeId(row.get(2)?),
            comment: row.get(3)?,
        });
    }
    Ok(bookmarks)
}

pub(super) fn update_bookmark_comment(
    conn: &Connection,
    id: i64,
    comment: &str,
) -> Result<(), StorageError> {
    conn.execute(
        "UPDATE bookmark_node SET comment = ?1 WHERE id = ?2",
        params![comment, id],
    )?;
    Ok(())
}

pub(super) fn update_bookmark(
    conn: &Connection,
    id: i64,
    category_id: Option<i64>,
    comment: Option<Option<&str>>,
) -> Result<(), StorageError> {
    let mut stmt = conn.prepare("SELECT category_id, comment FROM bookmark_node WHERE id = ?1")?;
    let mut rows = stmt.query(params![id])?;
    let Some(row) = rows.next()? else {
        return Ok(());
    };
    let current_category_id: i64 = row.get(0)?;
    let current_comment: Option<String> = row.get(1)?;
    let next_category_id = category_id.unwrap_or(current_category_id);
    let next_comment = match comment {
        Some(Some(value)) => Some(value.to_string()),
        Some(None) => None,
        None => current_comment,
    };

    conn.execute(
        "UPDATE bookmark_node SET category_id = ?1, comment = ?2 WHERE id = ?3",
        params![next_category_id, next_comment, id],
    )?;
    Ok(())
}

pub(super) fn delete_bookmark(conn: &Connection, id: i64) -> Result<(), StorageError> {
    conn.execute("DELETE FROM bookmark_node WHERE id = ?1", params![id])?;
    Ok(())
}
