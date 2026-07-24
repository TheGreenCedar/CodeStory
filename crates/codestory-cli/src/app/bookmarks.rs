use super::artifacts::{ensure_dot_only_for_trail, preflight_output_file};
use super::resolution::resolve_target_or_emit_ambiguity;
use crate::args::{
    BookmarkAction, BookmarkAddCommand, BookmarkAddOutput, BookmarkCommand, BookmarkListCommand,
    BookmarkListOutput, BookmarkOutput, BookmarkRemoveCommand, BookmarkRemoveOutput,
};
use crate::display;
use crate::output::emit;
use crate::runtime::{RuntimeContext, ensure_index_ready, map_api_error};
use anyhow::Context;
use anyhow::{Result, bail};
use codestory_contracts::api::{
    BookmarkCategoryDto, BookmarkDto, CreateBookmarkCategoryRequest, CreateBookmarkRequest,
    NodeKind,
};

pub(super) fn run_bookmark(cmd: BookmarkCommand) -> Result<()> {
    match cmd.action {
        BookmarkAction::Add(cmd) => run_bookmark_add(cmd),
        BookmarkAction::List(cmd) => run_bookmark_list(cmd),
        BookmarkAction::Remove(cmd) => run_bookmark_remove(cmd),
    }
}

fn run_bookmark_add(cmd: BookmarkAddCommand) -> Result<()> {
    ensure_dot_only_for_trail(cmd.format, "bookmark add")?;
    preflight_output_file(cmd.output_file.as_deref())?;
    let runtime = RuntimeContext::new(&cmd.project)?;
    let opened = runtime.ensure_open(cmd.refresh)?;
    ensure_index_ready(&opened, "bookmark add")?;
    let file_filter = cmd.target.file_filter();
    let target = resolve_target_or_emit_ambiguity(
        &runtime,
        cmd.target.selection()?,
        file_filter.as_deref(),
        cmd.format,
        cmd.output_file.as_deref(),
    )?;
    let category = ensure_bookmark_category(&runtime, &cmd.category)?;
    let bookmark = runtime
        .bookmarks
        .create_bookmark(CreateBookmarkRequest {
            category_id: category.id.clone(),
            node_id: target.selected.node_id.clone(),
            comment: cmd.comment.clone(),
        })
        .map_err(map_api_error)?;
    let output = BookmarkAddOutput {
        category,
        bookmark: bookmark_output(bookmark),
    };
    emit(
        cmd.format,
        &output,
        render_bookmark_add_markdown(&output),
        cmd.output_file.as_deref(),
    )
}

fn run_bookmark_list(cmd: BookmarkListCommand) -> Result<()> {
    ensure_dot_only_for_trail(cmd.format, "bookmark list")?;
    preflight_output_file(cmd.output_file.as_deref())?;
    let runtime = RuntimeContext::new(&cmd.project)?;
    let _summary = runtime.open_project_summary()?;
    let categories = runtime.bookmarks.list_categories().map_err(map_api_error)?;
    let category_id = cmd
        .category
        .as_deref()
        .map(|category| resolve_bookmark_category_id(&categories, category))
        .transpose()?;
    let bookmarks = runtime
        .bookmarks
        .list_bookmarks(category_id)
        .map_err(map_api_error)?
        .into_iter()
        .map(bookmark_output)
        .collect::<Vec<_>>();
    let output = BookmarkListOutput {
        categories,
        bookmarks,
    };
    emit(
        cmd.format,
        &output,
        render_bookmark_list_markdown(&output),
        cmd.output_file.as_deref(),
    )
}

fn run_bookmark_remove(cmd: BookmarkRemoveCommand) -> Result<()> {
    ensure_dot_only_for_trail(cmd.format, "bookmark remove")?;
    preflight_output_file(cmd.output_file.as_deref())?;
    let runtime = RuntimeContext::new(&cmd.project)?;
    let _summary = runtime.open_project_summary()?;
    let bookmark_id = parse_bookmark_db_id(&cmd.id, "bookmark_id")?;
    find_bookmark_by_id(&runtime, &cmd.id)?;
    runtime
        .bookmarks
        .delete_bookmark(bookmark_id)
        .map_err(map_api_error)?;
    let output = BookmarkRemoveOutput {
        removed_id: bookmark_id.to_string(),
    };
    emit(
        cmd.format,
        &output,
        render_bookmark_remove_markdown(&output),
        cmd.output_file.as_deref(),
    )
}

fn bookmark_output(bookmark: BookmarkDto) -> BookmarkOutput {
    let stale = bookmark.node_kind == NodeKind::UNKNOWN;
    BookmarkOutput { bookmark, stale }
}

fn parse_bookmark_db_id(raw: &str, field: &str) -> Result<i64> {
    let trimmed = raw.trim();
    trimmed
        .parse::<i64>()
        .with_context(|| format!("Invalid {field}: `{trimmed}`"))
}

fn resolve_bookmark_category_id(categories: &[BookmarkCategoryDto], raw: &str) -> Result<i64> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        bail!("Bookmark category cannot be empty.");
    }
    if let Ok(id) = trimmed.parse::<i64>()
        && categories
            .iter()
            .any(|category| category.id == id.to_string())
    {
        return Ok(id);
    }
    categories
        .iter()
        .find(|category| category.name.eq_ignore_ascii_case(trimmed))
        .map(|category| parse_bookmark_db_id(&category.id, "category_id"))
        .unwrap_or_else(|| bail!("Bookmark category not found: `{trimmed}`"))
}

fn ensure_bookmark_category(runtime: &RuntimeContext, raw: &str) -> Result<BookmarkCategoryDto> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        bail!("Bookmark category cannot be empty.");
    }
    let categories = runtime.bookmarks.list_categories().map_err(map_api_error)?;
    if let Ok(id) = trimmed.parse::<i64>()
        && let Some(category) = categories
            .iter()
            .find(|category| category.id == id.to_string())
    {
        return Ok(category.clone());
    }
    if let Some(category) = categories
        .iter()
        .find(|category| category.name.eq_ignore_ascii_case(trimmed))
    {
        return Ok(category.clone());
    }
    runtime
        .bookmarks
        .create_category(CreateBookmarkCategoryRequest {
            name: trimmed.to_string(),
        })
        .map_err(map_api_error)
}

fn find_bookmark_by_id(runtime: &RuntimeContext, raw_id: &str) -> Result<BookmarkDto> {
    let bookmark_id = parse_bookmark_db_id(raw_id, "bookmark_id")?;
    runtime
        .bookmarks
        .list_bookmarks(None)
        .map_err(map_api_error)?
        .into_iter()
        .find(|bookmark| bookmark.id == bookmark_id.to_string())
        .with_context(|| format!("Bookmark not found: {bookmark_id}"))
}

pub(super) fn load_bookmark_focus_by_id(
    runtime: &RuntimeContext,
    raw_id: &str,
) -> Result<BookmarkDto> {
    let bookmark_id = parse_bookmark_db_id(raw_id, "bookmark_id")?;
    let bookmark = find_bookmark_by_id(runtime, raw_id)?;
    if bookmark.node_kind == NodeKind::UNKNOWN {
        bail!(
            "Bookmark {bookmark_id} is stale: node {} is no longer present after reindex.",
            bookmark.node_id.0
        );
    }
    Ok(bookmark)
}

fn render_bookmark_add_markdown(output: &BookmarkAddOutput) -> String {
    let mut markdown = String::new();
    markdown.push_str("# Bookmark Added\n");
    markdown.push_str(&format!("- category: {}\n", output.category.name));
    markdown.push_str(&render_bookmark_row(&output.bookmark));
    markdown
}

fn render_bookmark_list_markdown(output: &BookmarkListOutput) -> String {
    let mut markdown = String::new();
    markdown.push_str("# Bookmarks\n");
    markdown.push_str("categories:\n");
    for category in &output.categories {
        markdown.push_str(&format!("- {}: {}\n", category.id, category.name));
    }
    markdown.push_str("bookmarks:\n");
    if output.bookmarks.is_empty() {
        markdown.push_str("- none\n");
    }
    for bookmark in &output.bookmarks {
        markdown.push_str(&render_bookmark_row(bookmark));
    }
    markdown
}

fn render_bookmark_remove_markdown(output: &BookmarkRemoveOutput) -> String {
    format!("# Bookmark Removed\n- removed_id: {}\n", output.removed_id)
}

fn render_bookmark_row(output: &BookmarkOutput) -> String {
    let bookmark = &output.bookmark;
    let stale = if output.stale { " stale=true" } else { "" };
    let file = bookmark
        .file_path
        .as_deref()
        .map(|path| format!(" path=`{}`", display::clean_path_string(path)))
        .unwrap_or_default();
    let comment = bookmark
        .comment
        .as_deref()
        .map(|comment| format!(" comment=`{}`", comment.replace('`', "'")))
        .unwrap_or_default();
    format!(
        "- id={} node={} label=`{}` kind={:?}{file}{comment}{stale}\n",
        bookmark.id, bookmark.node_id.0, bookmark.node_label, bookmark.node_kind
    )
}
