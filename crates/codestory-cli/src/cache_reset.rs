//! Guided derived-only cache reset.
//!
//! Forward-only migrations fail closed on a database written by a newer
//! CodeStory: an older binary can neither read it nor repair it through
//! `--refresh full`, which also fails closed. Failing closed is the
//! data-integrity-correct choice — the alternative destroys a newer database —
//! but until now the only recovery was out of band: delete the cache by hand,
//! or reinstall the newer release.
//!
//! This is the in-band recovery. It moves the derived cache into a quarantine
//! directory beside it, never deletes, never opens the database (so a
//! schema-too-new store is reachable), and preserves user-authored annotation
//! state in place. The reindex step that rebuilds what was quarantined is
//! named in the output.
//!
//! Every identity it moves and every identity it preserves comes from
//! `codestory_contracts::owned_artifacts`, so a future owned artifact cannot
//! be silently left behind — or silently destroyed — by this path.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use codestory_contracts::bounded_locks::{self, FileLockKind, LockDeadline};
use codestory_contracts::owned_artifacts;

use crate::args::{CacheResetCommand, CacheResetOutput};
use crate::output::emit;
use crate::runtime::RuntimeContext;

/// Wait budget for the promotion exclusion the reset holds.
///
/// A legitimate holder is a whole publication or promotion pass, so this
/// matches the publication budget every other waiter behind such a pass uses.
const RESET_LOCK_WAIT: Duration = bounded_locks::PUBLICATION_LOCK_WAIT;

pub(crate) fn run_cache_reset(cmd: CacheResetCommand) -> Result<()> {
    crate::ensure_dot_only_for_trail(cmd.format, "cache reset")?;
    crate::preflight_output_file(cmd.output_file.as_deref())?;
    // clap requires exactly one of --confirm/--dry-run, so this is a guard
    // against a future argument edit silently making both optional.
    if cmd.confirm == cmd.dry_run {
        bail!("Pass exactly one of --confirm or --dry-run to `cache reset`.");
    }
    if !cmd.derived_only {
        bail!("`cache reset` only supports --derived-only.");
    }
    let runtime = RuntimeContext::new_inspect_only(&cmd.project)?;
    let output = plan_and_maybe_apply(&runtime.project_root, &runtime.storage_path, cmd.confirm)?;
    let markdown = render_cache_reset_markdown(&output);
    emit(cmd.format, &output, markdown, cmd.output_file.as_deref())
}

fn plan_and_maybe_apply(
    project_root: &Path,
    storage_path: &Path,
    apply: bool,
) -> Result<CacheResetOutput> {
    let project = crate::display::clean_path_string(&project_root.to_string_lossy());
    let plan = derived_reset_plan(storage_path);
    let quarantine_dir =
        owned_artifacts::derived_reset_quarantine_root(storage_path).join(quarantine_slot_name());
    let preserved = present_annotation_identities(storage_path);
    let mut output = CacheResetOutput {
        project,
        storage_path: crate::display::clean_path_string(&storage_path.to_string_lossy()),
        scope: "derived_only",
        applied: false,
        quarantine_dir: crate::display::clean_path_string(&quarantine_dir.to_string_lossy()),
        quarantined: plan
            .iter()
            .map(|path| display_name(storage_path, path))
            .collect(),
        preserved: preserved
            .iter()
            .map(|path| display_name(storage_path, path))
            .collect(),
        next_commands: reset_next_commands(&output_project_arg(project_root)),
    };
    if !apply {
        return Ok(output);
    }
    let moved = apply_derived_reset(storage_path, &plan, &quarantine_dir)?;
    output.applied = true;
    output.quarantined = moved
        .iter()
        .map(|path| display_name(storage_path, path))
        .collect();
    // Re-observe rather than trusting the pre-move plan: the whole point of
    // the command is that annotation state is still there afterwards.
    output.preserved = present_annotation_identities(storage_path)
        .iter()
        .map(|path| display_name(storage_path, path))
        .collect();
    Ok(output)
}

/// Derived identities that currently exist, in a stable order.
fn derived_reset_plan(storage_path: &Path) -> Vec<PathBuf> {
    let mut plan: Vec<PathBuf> = owned_artifacts::derived_reset_file_identities(storage_path)
        .into_iter()
        .filter(|path| path.is_file() || path.is_symlink())
        .collect();
    plan.extend(
        owned_artifacts::derived_reset_directory_identities(storage_path)
            .into_iter()
            .filter(|path| path.is_dir()),
    );
    plan.sort();
    plan.dedup();
    plan
}

fn present_annotation_identities(storage_path: &Path) -> Vec<PathBuf> {
    let cache_root = storage_path.parent().unwrap_or_else(|| Path::new("."));
    let mut preserved: Vec<PathBuf> = owned_artifacts::annotation_owned_file_identities(cache_root)
        .into_iter()
        .filter(|path| path.exists())
        .collect();
    preserved.sort();
    preserved
}

fn apply_derived_reset(
    storage_path: &Path,
    plan: &[PathBuf],
    quarantine_dir: &Path,
) -> Result<Vec<PathBuf>> {
    let cache_root = storage_path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(cache_root)
        .with_context(|| format!("create cache root {}", cache_root.display()))?;
    let lock_path = owned_artifacts::promotion_sibling_path(
        storage_path,
        owned_artifacts::PROMOTION_LOCK_SUFFIX,
    );
    let lock_file = fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .with_context(|| format!("open promotion lock {}", lock_path.display()))?;
    bounded_locks::acquire_with_deadline(
        &lock_file,
        FileLockKind::Exclusive,
        LockDeadline::after(RESET_LOCK_WAIT),
        None,
    )
    .with_context(|| {
        format!(
            "acquire the promotion lock for the derived cache reset at {}",
            lock_path.display()
        )
    })?;
    let result = move_plan_into_quarantine(storage_path, plan, quarantine_dir);
    let _ = bounded_locks::release(&lock_file);
    result
}

fn move_plan_into_quarantine(
    storage_path: &Path,
    plan: &[PathBuf],
    quarantine_dir: &Path,
) -> Result<Vec<PathBuf>> {
    fs::create_dir_all(quarantine_dir)
        .with_context(|| format!("create quarantine directory {}", quarantine_dir.display()))?;
    let mut moved = Vec::new();
    for source in plan {
        if !source.exists() {
            continue;
        }
        let destination = quarantine_dir.join(display_name(storage_path, source));
        fs::rename(source, &destination).with_context(|| {
            format!(
                "quarantine {} into {}",
                source.display(),
                destination.display()
            )
        })?;
        moved.push(source.clone());
    }
    Ok(moved)
}

/// Name a cache-root-relative identity for both the report and the quarantine
/// destination, so the quarantine is a flat mirror of what was moved.
fn display_name(storage_path: &Path, path: &Path) -> String {
    let cache_root = storage_path.parent().unwrap_or_else(|| Path::new("."));
    path.strip_prefix(cache_root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

/// Name one quarantine slot so repeated resets never collide and the newest
/// sorts last.
fn quarantine_slot_name() -> String {
    let epoch_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or_default();
    format!("{epoch_ms:013}")
}

fn output_project_arg(project_root: &Path) -> String {
    crate::display::clean_path_string(&project_root.to_string_lossy())
}

fn reset_next_commands(project: &str) -> Vec<String> {
    vec![
        format!("codestory-cli index --project \"{project}\" --refresh full"),
        format!("codestory-cli retrieval index --project \"{project}\" --refresh full"),
        format!("codestory-cli doctor --project \"{project}\" --format markdown"),
    ]
}

fn render_cache_reset_markdown(output: &CacheResetOutput) -> String {
    let mut markdown = format!(
        "# Derived cache reset\n\n- project: `{}`\n- storage_path: `{}`\n- scope: `{}`\n- applied: {}\n- quarantine_dir: `{}`\n",
        output.project, output.storage_path, output.scope, output.applied, output.quarantine_dir
    );
    markdown.push_str("\n## Quarantined derived state\n\n");
    if output.quarantined.is_empty() {
        markdown.push_str("- none present\n");
    } else {
        for name in &output.quarantined {
            markdown.push_str(&format!("- `{name}`\n"));
        }
    }
    markdown.push_str("\n## Preserved user state\n\n");
    if output.preserved.is_empty() {
        markdown.push_str("- no annotation sidecar present\n");
    } else {
        for name in &output.preserved {
            markdown.push_str(&format!("- `{name}`\n"));
        }
    }
    if !output.applied {
        markdown.push_str(
            "\nPlan only: nothing was moved. Rerun with `--confirm` instead of `--dry-run` to quarantine.\n",
        );
    }
    markdown.push_str("\n## Reindex\n\n");
    for command in &output.next_commands {
        markdown.push_str(&format!("- `{command}`\n"));
    }
    markdown
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    struct CacheFixture {
        _root: TempDir,
        project_root: PathBuf,
        storage_path: PathBuf,
        annotations: PathBuf,
    }

    /// A cache root holding derived core state plus a user annotation sidecar.
    fn seed_cache() -> CacheFixture {
        let root = TempDir::new().expect("fixture root");
        let project_root = root.path().join("project");
        let cache_root = root.path().join("cache");
        fs::create_dir_all(&project_root).expect("create project");
        fs::create_dir_all(&cache_root).expect("create cache root");
        let storage_path = cache_root.join("codestory.db");
        fs::write(&storage_path, b"core database bytes").expect("write core database");
        fs::write(cache_root.join("codestory.db-wal"), b"wal").expect("write wal");
        fs::write(
            cache_root.join("codestory.sqlite.backup"),
            b"rollback backup",
        )
        .expect("write rollback backup");
        fs::write(cache_root.join("local-refresh-status.json"), b"{}")
            .expect("write refresh state");
        fs::create_dir_all(cache_root.join("codestory.search")).expect("create search tree");
        fs::write(cache_root.join("codestory.search").join("shard"), b"shard")
            .expect("write search shard");
        let annotations = cache_root.join("annotations.sqlite3");
        fs::write(&annotations, b"user bookmarks").expect("write annotation sidecar");
        fs::write(
            cache_root.join("annotations.sqlite3-wal"),
            b"annotation wal",
        )
        .expect("write annotation wal");
        CacheFixture {
            _root: root,
            project_root,
            storage_path,
            annotations,
        }
    }

    #[test]
    fn confirmed_reset_quarantines_derived_state_and_preserves_annotations() {
        let fixture = seed_cache();

        let output = plan_and_maybe_apply(&fixture.project_root, &fixture.storage_path, true)
            .expect("reset");

        assert!(output.applied);
        assert!(
            !fixture.storage_path.exists(),
            "the core database must leave the live cache location"
        );
        assert!(
            !fixture
                .storage_path
                .parent()
                .expect("cache root")
                .join("codestory.search")
                .exists(),
            "the derived search tree must leave the live cache location"
        );
        assert_eq!(
            fs::read(&fixture.annotations).expect("annotation sidecar still readable"),
            b"user bookmarks",
            "user-authored annotations must survive a derived-only reset byte for byte"
        );
        assert!(
            fixture
                .annotations
                .parent()
                .expect("cache root")
                .join("annotations.sqlite3-wal")
                .exists(),
            "annotation SQLite siblings must survive too"
        );
        let quarantined = Path::new(&output.quarantine_dir);
        assert_eq!(
            fs::read(quarantined.join("codestory.db")).expect("quarantined core database"),
            b"core database bytes",
            "the reset must move derived state, not destroy it"
        );
        assert!(quarantined.join("codestory.search").join("shard").is_file());
        assert_eq!(
            output.preserved,
            vec![
                "annotations.sqlite3".to_string(),
                "annotations.sqlite3-wal".to_string()
            ]
        );
        assert!(
            output
                .next_commands
                .iter()
                .any(|command| command.contains("--refresh full")),
            "the reset must name the reindex step: {:?}",
            output.next_commands
        );
    }

    #[test]
    fn dry_run_reports_the_plan_without_moving_anything() {
        let fixture = seed_cache();

        let output = plan_and_maybe_apply(&fixture.project_root, &fixture.storage_path, false)
            .expect("plan");

        assert!(!output.applied);
        assert!(
            fixture.storage_path.is_file(),
            "a dry run must leave the derived cache in place"
        );
        assert!(
            !Path::new(&output.quarantine_dir).exists(),
            "a dry run must not create the quarantine directory"
        );
        assert!(
            output.quarantined.contains(&"codestory.db".to_string()),
            "the plan must name the derived state it would move: {:?}",
            output.quarantined
        );
        assert!(
            !output
                .quarantined
                .iter()
                .any(|name| name.starts_with("annotations.sqlite3")),
            "the plan must never list user annotation state: {:?}",
            output.quarantined
        );
    }

    #[test]
    fn reset_does_not_open_or_migrate_the_core_database() {
        // A schema-too-new database is the exact case this command exists for,
        // so it must be reachable without a store open. Bytes that no SQLite
        // build can open stand in for a database this binary must not touch.
        let fixture = seed_cache();
        fs::write(&fixture.storage_path, b"not a sqlite database at all")
            .expect("write unopenable core database");

        let output = plan_and_maybe_apply(&fixture.project_root, &fixture.storage_path, true)
            .expect("reset");

        assert!(output.applied);
        assert_eq!(
            fs::read(Path::new(&output.quarantine_dir).join("codestory.db"))
                .expect("quarantined core database"),
            b"not a sqlite database at all",
            "the reset must move the database untouched, not open or migrate it"
        );
    }
}
