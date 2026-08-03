//! Freshness verdicts under an armed filesystem observer.
//!
//! The property under test is one-directional: observation may turn a `Fresh` verdict the scan
//! could not have known was wrong into `Stale`, and may never do anything else. Every case where
//! the observer knows less than it would like — dropped events, an unsupported filesystem, no
//! observer at all — must leave the EV-7 scan verdict exactly as it was.

use super::{hybrid_test_env, test_sidecar_runtime_from_env};
use crate::index_freshness::{
    FreshnessObservation, FreshnessObservationPolicy, index_freshness_from_storage_with_policy,
};
use crate::{AppController, SourceIndexPolicy, Storage, WorkspaceManifest};
use codestory_contracts::api::{
    IndexFreshnessChangeKindDto, IndexFreshnessDto, IndexFreshnessStatusDto, IndexMode,
};
use codestory_workspace::filesystem_observer::{
    FilesystemObserverSession, MutationScope, ObservedFilesystemEvent, ObserverEventSource,
};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use tempfile::{TempDir, tempdir};

/// An event source whose script runs on each drain, so a test can move the working tree at the
/// exact point in the window a real writer would have.
struct ScriptedObserver {
    drains: usize,
    script: Arc<Mutex<dyn FnMut(usize) -> Vec<ObservedFilesystemEvent> + Send>>,
}

impl ObserverEventSource for ScriptedObserver {
    fn drain(&mut self) -> Vec<ObservedFilesystemEvent> {
        let index = self.drains;
        self.drains += 1;
        let mut script = self
            .script
            .lock()
            .expect("scripted observer script poisoned");
        (script)(index)
    }
}

pub(crate) fn scripted_session(
    root: &Path,
    script: impl FnMut(usize) -> Vec<ObservedFilesystemEvent> + Send + 'static,
) -> FilesystemObserverSession {
    FilesystemObserverSession::arm_with_source(
        root,
        Box::new(ScriptedObserver {
            drains: 0,
            script: Arc::new(Mutex::new(script)),
        }),
    )
}

/// A published project with one indexed source file whose scan verdict is `Fresh`.
struct ObservedProject {
    workspace: TempDir,
    storage_path: PathBuf,
    controller: AppController,
    source: PathBuf,
}

impl ObservedProject {
    fn publish() -> Self {
        let workspace = tempdir().expect("workspace");
        let source_directory = workspace.path().join("src");
        std::fs::create_dir(&source_directory).expect("source directory");
        let source = source_directory.join("lib.rs");
        std::fs::write(&source, "pub fn published() {}\n").expect("published source");
        let storage_path = workspace.path().join(".cache/codestory.db");
        let controller = AppController::new_with_config(test_sidecar_runtime_from_env());
        controller
            .open_project_summary_with_storage_path(
                workspace.path().to_path_buf(),
                storage_path.clone(),
            )
            .expect("open project summary");
        controller
            .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
            .expect("baseline publication");
        Self {
            workspace,
            storage_path,
            controller,
            source,
        }
    }

    fn root(&self) -> &Path {
        self.workspace.path()
    }

    fn freshness(&self, observation: FreshnessObservation<'_>) -> IndexFreshnessDto {
        let manifest = WorkspaceManifest::open_with_storage_owned_exclusions(
            self.workspace.path().to_path_buf(),
            &self.storage_path,
        )
        .expect("workspace manifest");
        let storage = Storage::open(&self.storage_path).expect("published storage");
        index_freshness_from_storage_with_policy(
            self.workspace.path(),
            &manifest,
            &storage,
            &SourceIndexPolicy::default(),
            observation,
        )
    }
}

fn mutated(path: &Path, scope: MutationScope) -> ObservedFilesystemEvent {
    ObservedFilesystemEvent::Mutated {
        path: path.to_path_buf(),
        scope,
    }
}

#[test]
fn an_unobserved_scan_reports_the_published_tree_as_fresh() {
    let _env = hybrid_test_env();
    let project = ObservedProject::publish();
    let freshness = project.freshness(FreshnessObservation::Unobserved);
    assert_eq!(freshness.status, IndexFreshnessStatusDto::Fresh);
    assert_eq!(freshness.reason, None);
}

#[test]
fn a_write_that_races_the_scan_is_reported_stale_instead_of_fresh() {
    let _env = hybrid_test_env();
    let project = ObservedProject::publish();
    let racing_path = project.source.clone();
    // The scan runs inside the window and finds the published bytes. The write lands afterwards,
    // which is exactly the state a writer that beat the walker to a directory leaves behind.
    let session = scripted_session(project.root(), move |drain| {
        if drain == 0 {
            return Vec::new();
        }
        std::fs::write(&racing_path, "pub fn racing() {}\n").expect("racing write");
        vec![mutated(&racing_path, MutationScope::File)]
    });

    let freshness = project.freshness(FreshnessObservation::Observed(&session));
    assert_eq!(
        freshness.status,
        IndexFreshnessStatusDto::Stale,
        "a scan that raced a tracked write must not authorise its own verdict"
    );
    assert_eq!(
        freshness.reason.as_deref(),
        Some("source_changed_during_freshness_scan_observed_by_injected")
    );
    assert_eq!(freshness.changed_file_count, 1);
    assert_eq!(
        freshness
            .samples
            .iter()
            .map(|sample| (sample.kind, sample.path.as_str()))
            .collect::<Vec<_>>(),
        vec![(
            IndexFreshnessChangeKindDto::Changed,
            Path::new("src").join("lib.rs").to_string_lossy().as_ref()
        )]
    );
}

#[test]
fn the_scan_runs_inside_the_window_the_observer_sealed() {
    let _env = hybrid_test_env();
    let project = ObservedProject::publish();
    let racing_path = project.source.clone();
    // Every other case in this file scripts events by drain index, and a drain index cannot tell
    // arm-then-scan apart from scan-then-arm: the seal drains either way. This one moves the
    // working tree at the instant the window opens and reports nothing at all, so the only thing
    // that can turn the verdict is the scan reading bytes the window opened over. A scan that
    // already finished before the window opened reads the published bytes and answers `Fresh`.
    let session = scripted_session(project.root(), move |drain| {
        if drain == 0 {
            std::fs::write(&racing_path, "pub fn wrote_at_the_seam() {}\n").expect("seam write");
        }
        Vec::new()
    });

    let freshness = project.freshness(FreshnessObservation::Observed(&session));
    assert_eq!(
        freshness.status,
        IndexFreshnessStatusDto::Stale,
        "the scan must run inside the window, so a write that lands as the window opens is one \
         the scan itself has to read"
    );
    assert_eq!(freshness.changed_file_count, 1);
    assert_eq!(
        freshness.reason, None,
        "the scan caught this one on its own; nothing was escalated"
    );
    assert_eq!(
        freshness
            .samples
            .iter()
            .map(|sample| sample.path.as_str())
            .collect::<Vec<_>>(),
        vec![Path::new("src").join("lib.rs").to_string_lossy().as_ref()]
    );
}

#[test]
fn a_dirty_path_whose_bytes_did_not_move_stays_fresh() {
    let _env = hybrid_test_env();
    let project = ObservedProject::publish();
    let quiet_path = project.source.clone();
    // Notifiers report metadata touches and rewrites of identical bytes. Rehashing the named path
    // against the planner's own predicate is what keeps those from reading as drift.
    let session = scripted_session(project.root(), move |drain| {
        if drain == 0 {
            return Vec::new();
        }
        vec![mutated(&quiet_path, MutationScope::File)]
    });

    let freshness = project.freshness(FreshnessObservation::Observed(&session));
    assert_eq!(freshness.status, IndexFreshnessStatusDto::Fresh);
    assert_eq!(freshness.reason, None);
    assert_eq!(freshness.changed_file_count, 0);
}

/// The operation-scoped freshness memo and this observer meet inside the escalation, which
/// settles a dirty path by re-applying the planner's own predicate — and that predicate is
/// memoized. The scan this window just sealed recorded a verdict for the very path the observer
/// is reporting, taken before the racing write. If the memo answered here the escalation would
/// agree with the verdict it exists to falsify, and the one drift shape metadata cannot see would
/// be served.
#[test]
fn a_same_mtime_same_length_race_escalates_from_content_inside_an_armed_memo_scope() {
    let _env = hybrid_test_env();
    let project = ObservedProject::publish();
    let racing_path = project.source.clone();
    let published = std::fs::metadata(&racing_path).expect("stat the published source");
    let published_len = published.len();
    let published_mtime = published.modified().expect("published modification time");
    let session = scripted_session(project.root(), move |drain| {
        if drain == 0 {
            return Vec::new();
        }
        // Same byte length, same modification time, different bytes: exactly the drift no
        // metadata comparison can see, landing after the scan already answered `Fresh`.
        std::fs::write(&racing_path, "pub fn publishee() {}\n").expect("racing write");
        std::fs::File::options()
            .write(true)
            .open(&racing_path)
            .expect("reopen the raced source")
            .set_modified(published_mtime)
            .expect("restore the modification time");
        let observed = std::fs::metadata(&racing_path).expect("stat the raced source");
        assert_eq!(
            observed.len(),
            published_len,
            "the race must preserve the byte length to exercise the guard"
        );
        assert_eq!(
            observed.modified().expect("raced modification time"),
            published_mtime,
            "the race must preserve the modification time to exercise the guard"
        );
        vec![mutated(&racing_path, MutationScope::File)]
    });

    let _memo = codestory_workspace::SourceFreshnessScope::enter();
    // Warm the memo the way a public operation's pre-body admission does.
    assert_eq!(
        project.freshness(FreshnessObservation::Unobserved).status,
        IndexFreshnessStatusDto::Fresh
    );

    let freshness = project.freshness(FreshnessObservation::Observed(&session));
    assert_eq!(
        freshness.status,
        IndexFreshnessStatusDto::Stale,
        "the escalation has to re-read content; a memoized verdict only describes the instant \
         before the race"
    );
    assert_eq!(
        freshness.reason.as_deref(),
        Some("source_changed_during_freshness_scan_observed_by_injected")
    );
    assert_eq!(freshness.changed_file_count, 1);
}

#[test]
fn churn_outside_the_admitted_tree_never_escalates() {
    let _env = hybrid_test_env();
    let project = ObservedProject::publish();
    let build_output = project.root().join("target");
    let git_directory = project.root().join(".git");
    let attachments = project.root().join("attachments");
    // A build and a checkout running alongside a scan must not leave freshness permanently
    // escalated over paths discovery never admitted. `attachments/` is the case the observer's own
    // scope filter does not cover: it is an ordinary directory the session accounts for, and only
    // the plan knows discovery admitted nothing inside it.
    let session = scripted_session(project.root(), move |drain| {
        if drain == 0 {
            return Vec::new();
        }
        vec![
            mutated(&build_output, MutationScope::Directory),
            mutated(
                &build_output.join("debug").join("main.rs"),
                MutationScope::File,
            ),
            mutated(&git_directory, MutationScope::Directory),
            mutated(&git_directory.join("index"), MutationScope::File),
            mutated(&attachments, MutationScope::Directory),
            mutated(&attachments.join("photo.bin"), MutationScope::File),
        ]
    });

    let freshness = project.freshness(FreshnessObservation::Observed(&session));
    assert_eq!(freshness.status, IndexFreshnessStatusDto::Fresh);
    assert_eq!(freshness.reason, None);
}

#[test]
fn a_directory_event_inside_the_admitted_tree_escalates_without_a_broad_pass() {
    let _env = hybrid_test_env();
    let project = ObservedProject::publish();
    let source_directory = project.root().join("src");
    // A create, rename, or removal names no file the caller can rehash. Escalating is the only
    // answer that stays correct without silently re-walking the repository.
    let session = scripted_session(project.root(), move |drain| {
        if drain == 0 {
            return Vec::new();
        }
        vec![mutated(&source_directory, MutationScope::Directory)]
    });

    let freshness = project.freshness(FreshnessObservation::Observed(&session));
    assert_eq!(freshness.status, IndexFreshnessStatusDto::Stale);
    assert_eq!(
        freshness
            .samples
            .iter()
            .map(|sample| sample.path.as_str())
            .collect::<Vec<_>>(),
        vec!["src"]
    );
}

#[test]
fn a_window_that_lost_events_falls_back_to_the_scan_verdict() {
    let _env = hybrid_test_env();
    let project = ObservedProject::publish();
    let racing_path = project.source.clone();
    // The same racing write as the escalation case, but the notifier admits it dropped events.
    // EV-7 is the floor: the scan verdict stands untouched, which is the availability cost the
    // typed-unknown fallback deliberately accepts rather than forcing a broad pass per operation.
    let session = scripted_session(project.root(), move |drain| {
        if drain == 0 {
            return Vec::new();
        }
        std::fs::write(&racing_path, "pub fn racing() {}\n").expect("racing write");
        vec![
            ObservedFilesystemEvent::CoverageLost {
                detail: "queue overflow".to_string(),
            },
            mutated(&racing_path, MutationScope::File),
        ]
    });

    let freshness = project.freshness(FreshnessObservation::Observed(&session));
    assert_eq!(
        freshness.status,
        IndexFreshnessStatusDto::Fresh,
        "an indeterminate window may not manufacture a verdict the observer did not prove"
    );
    assert_eq!(freshness.reason, None);
}

#[test]
fn a_session_that_lost_coverage_stamps_no_certainty_on_a_ready_lease() {
    let _env = hybrid_test_env();
    let project = ObservedProject::publish();
    let covered = Arc::new(AtomicBool::new(true));
    let gate = Arc::clone(&covered);
    let session = scripted_session(project.root(), move |_| {
        if gate.load(std::sync::atomic::Ordering::Acquire) {
            return Vec::new();
        }
        vec![ObservedFilesystemEvent::CoverageLost {
            detail: "queue overflow".to_string(),
        }]
    });
    project
        .controller
        .install_source_observer_for_test(project.root(), Arc::new(session));
    assert!(
        project
            .controller
            .observed_source_epoch(project.root())
            .is_some(),
        "a covering session is what lets a lease carry a falsifiable source claim"
    );

    // Losing coverage does not degrade the verdict — the scan verdict is the declared floor — but
    // it must degrade the *lease*, which would otherwise keep quoting an epoch the observer can no
    // longer stand behind.
    covered.store(false, std::sync::atomic::Ordering::Release);
    assert_eq!(
        project.controller.observed_source_epoch(project.root()),
        None,
        "an epoch from a session that admits it missed something is not evidence"
    );
}

#[test]
fn serving_reads_arm_an_observer_and_observational_reads_do_not() {
    let _env = hybrid_test_env();
    let project = ObservedProject::publish();

    project
        .controller
        .index_freshness()
        .expect("observational freshness");
    project
        .controller
        .open_project_summary_with_storage_path(
            project.root().to_path_buf(),
            project.storage_path.clone(),
        )
        .expect("observational project summary");
    assert_eq!(
        project.controller.source_observer_requests_for_test(),
        0,
        "status, doctor, and summary reads observe what exists and never create observers"
    );

    let service = crate::services::PublicOperationService::new(project.controller.clone());
    service
        .run_with_cancel("symbols", Arc::new(AtomicBool::new(false)), || Ok(()))
        .expect("public operation over a fresh publication");
    assert!(
        project.controller.source_observer_requests_for_test() > 0,
        "the admission that authorises serving must be the one that arms the observer"
    );
}

/// A session that escalates every window it seals, without moving a single byte on disk.
///
/// A directory-scoped mutation names no file to rehash, so the scan verdict stays `Fresh` on its
/// own and the observer is the only thing that can refuse. That is what makes these tests measure
/// the consequence rather than the arming: nothing but the escalation can turn them red.
fn escalating_session(root: &Path, quiet_drains: usize) -> FilesystemObserverSession {
    let source_directory = root.join("src");
    scripted_session(root, move |drain| {
        if drain < quiet_drains {
            return Vec::new();
        }
        vec![mutated(&source_directory, MutationScope::Directory)]
    })
}

#[test]
fn an_escalated_verdict_refuses_the_public_operation_before_its_body_runs() {
    let _env = hybrid_test_env();
    let project = ObservedProject::publish();
    let session = escalating_session(project.root(), 1);
    project
        .controller
        .install_source_observer_for_test(project.root(), Arc::new(session));

    let service = crate::services::PublicOperationService::new(project.controller.clone());
    let builds = std::cell::Cell::new(0usize);
    let error = service
        .run_with_cancel("symbols", Arc::new(AtomicBool::new(false)), || {
            builds.set(builds.get() + 1);
            Ok(())
        })
        .expect_err("an observer-escalated verdict must refuse the operation, not just be counted");

    assert_eq!(
        error.code, "project_unavailable",
        "the admission before the operation body is the one that has to refuse"
    );
    assert_eq!(
        builds.get(),
        0,
        "a refusal that lets the response get built is not a refusal"
    );
}

#[test]
fn an_escalated_verdict_refuses_the_public_operation_after_its_body_runs() {
    let _env = hybrid_test_env();
    let project = ObservedProject::publish();
    // Quiet across the first admission's window (its arm and its seal), then escalating: the
    // mutation lands while the operation body is running, which only the read after the body can
    // catch.
    let session = escalating_session(project.root(), 2);
    project
        .controller
        .install_source_observer_for_test(project.root(), Arc::new(session));

    let service = crate::services::PublicOperationService::new(project.controller.clone());
    let builds = std::cell::Cell::new(0usize);
    let error = service
        .run_with_cancel("symbols", Arc::new(AtomicBool::new(false)), || {
            builds.set(builds.get() + 1);
            Ok(())
        })
        .expect_err("a response built over a tree that moved under it must not be served");

    assert_eq!(error.code, "project_unavailable");
    assert_eq!(
        builds.get(),
        1,
        "the first attempt ran and was refused after the fact; its bounded retry was refused \
         before running again"
    );
}

#[test]
fn one_session_serves_every_observed_read_of_the_same_root() {
    let _env = hybrid_test_env();
    let project = ObservedProject::publish();
    let root = project.root().to_path_buf();
    let first = project
        .controller
        .source_observer_session(&root)
        .expect("a local temporary directory must be observable");
    let second = project
        .controller
        .source_observer_session(&root)
        .expect("the armed session must be reused");
    assert_eq!(
        first.identity(),
        second.identity(),
        "re-arming per read would pay the recursive watch cost on every operation"
    );
    assert!(Arc::ptr_eq(&first, &second));
}

#[test]
fn the_observed_policy_is_what_reaches_the_freshness_check() {
    let _env = hybrid_test_env();
    let project = ObservedProject::publish();
    let racing_path = project.source.clone();
    let session = scripted_session(project.root(), move |drain| {
        if drain == 0 {
            return Vec::new();
        }
        std::fs::write(&racing_path, "pub fn racing() {}\n").expect("racing write");
        vec![mutated(&racing_path, MutationScope::File)]
    });
    project
        .controller
        .install_source_observer_for_test(project.root(), Arc::new(session));

    let unobserved = project
        .controller
        .index_freshness_uncached(FreshnessObservationPolicy::Unobserved)
        .expect("unobserved freshness");
    assert_eq!(
        unobserved.status,
        IndexFreshnessStatusDto::Fresh,
        "the unobserved policy must not reach the armed session"
    );

    let observed = project
        .controller
        .index_freshness_uncached(FreshnessObservationPolicy::ObserveSourceRoot)
        .expect("observed freshness");
    assert_eq!(
        observed.status,
        IndexFreshnessStatusDto::Stale,
        "the controller must hand the armed session to the freshness check"
    );
    assert_eq!(
        observed.reason.as_deref(),
        Some("source_changed_during_freshness_scan_observed_by_injected")
    );
}
