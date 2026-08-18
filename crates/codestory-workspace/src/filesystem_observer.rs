//! Cross-platform filesystem observation that may only ever upgrade certainty.
//!
//! A freshness check that walks the working tree and compares content hashes answers a question
//! about a *moving* target: a source file written after the walk passed its directory is reported
//! as unchanged. This module closes that window by arming a platform change notifier **before**
//! the scan starts and sealing it after the scan finishes, so the check learns whether anything
//! mutated while it was looking.
//!
//! The one invariant every part of this module exists to hold is that observation may only ever
//! *upgrade* certainty. It can prove a scan raced a mutation; it can never prove a scan did not.
//! Whenever the observer cannot account for the whole window — the backend refused to arm, the
//! root sits on a filesystem whose notifications are not trustworthy (WSL interop, network
//! mounts), the kernel queue dropped events, or the accumulation budget was exhausted — the
//! window seals [`FilesystemObserverCoverage::Indeterminate`] with a typed gap and the caller
//! keeps whatever verdict it computed without the observer. That fallback is the permanent floor
//! beneath this module, not a temporary state.
//!
//! Backend losses are sticky. Once a notifier has told a session it missed something, that
//! session can never seal `Proven` again: a backend that dropped one event has no way to say what
//! it dropped, and on inotify the descriptors for subtrees created during the overflow were never
//! installed. Recovering certainty requires arming a new session, which carries a new identity.
//!
//! Losses the session inflicts on *itself* are not sticky, because it knows exactly what they
//! cost. An overflowed per-window accumulator and an overlapping window each seal one window
//! indeterminate and clear when every window closes: the accumulator is per-window by
//! construction and the next caller re-reads the tree anyway, so nothing carries forward.
//!
//! Churn is filtered on arrival by [`ObservedScopeFilter`] rather than downstream, so a build
//! writing `target/` cannot spend the observation budget of a scan it was never going to affect.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, TryRecvError};

use uuid::Uuid;

/// Monotone count of mutations one armed session has absorbed since arming.
///
/// Two observations of the same session at the same epoch saw the same filesystem: the counter
/// advances on every mutation the backend reports, so an unchanged epoch is a positive statement
/// that nothing was observed in between.
pub type ObserverEpoch = u64;

/// Paths one session accumulates inside a single window before it declares a hole.
///
/// The bound exists so a runaway writer cannot grow observer state without limit. Exhausting it
/// seals *that* window indeterminate and the caller falls back — but unlike a dropped kernel
/// event it is not a permanent loss. The accumulator is per-window by construction, and the next
/// window's caller re-reads the tree anyway, so the session recovers as soon as a window closes.
pub const FILESYSTEM_OBSERVER_DIRTY_PATH_CAP: usize = 4_096;

/// Directory names whose churn under a watched root can never be admitted source.
///
/// A recursive watch over a project root sees everything: a build writing `target/`, a checkout
/// rewriting `.git/`, a package install unpacking `node_modules/`. None of it can ever reach a
/// freshness verdict, because discovery excludes those trees by default — so charging them
/// against the observation budget spends the whole window's accounting on paths the caller is
/// about to throw away. The names mirror `default_source_exclude_patterns`, which is what
/// discovery actually applies.
const OBSERVER_IGNORED_DIRECTORY_NAMES: &[&str] =
    &[".git", "node_modules", "target", "dist", "build"];

/// Which notifier produced a session's events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilesystemObserverBackend {
    /// The platform change notifier selected for this host.
    Native,
    /// A caller-supplied event source. Only reachable from tests.
    Injected,
}

impl FilesystemObserverBackend {
    /// Stable machine-readable identity for lease and diagnostic text.
    pub fn id(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::Injected => "injected",
        }
    }
}

/// Stable identity of one armed observation session.
///
/// The session id changes on every arming, so a caller that recorded an identity can tell a
/// re-armed observer (which knows nothing about the window before it) from the one it trusted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilesystemObserverIdentity {
    backend: FilesystemObserverBackend,
    root: PathBuf,
    session_id: String,
}

impl FilesystemObserverIdentity {
    /// The notifier behind this session.
    pub fn backend(&self) -> FilesystemObserverBackend {
        self.backend
    }

    /// The root this session watches, exactly as the caller spelled it.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Unique per arming. A new session never reuses an earlier id.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }
}

/// A filesystem whose change notification cannot carry a freshness claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnsupportedFilesystemKind {
    /// A remote mount. Notifications describe local activity only, so remote writes are invisible.
    NetworkMount,
    /// A Windows Subsystem for Linux interop mount, where events cross a translation layer.
    WindowsSubsystemForLinuxInterop,
    /// A userspace filesystem whose notification support is defined by the driver, not the kernel.
    UserspaceMount,
}

impl UnsupportedFilesystemKind {
    /// Stable machine-readable identity for gap text and typed output fields.
    pub fn id(self) -> &'static str {
        match self {
            Self::NetworkMount => "network_mount",
            Self::WindowsSubsystemForLinuxInterop => "wsl_interop_mount",
            Self::UserspaceMount => "userspace_mount",
        }
    }
}

/// Why an observation window proves nothing about drift.
///
/// Every variant means the same thing to a caller — fall back to the unobserved verdict — but
/// they are distinct because an operator diagnosing "why is this repository never observed"
/// needs to know whether the notifier is missing, the filesystem is unsupported, or the host is
/// simply producing more churn than the session budget allows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilesystemObserverGap {
    /// The platform notifier could not be created or could not watch the root.
    BackendUnavailable { detail: String },
    /// The root sits on a filesystem whose change notification is not trustworthy.
    UnsupportedFilesystem {
        kind: UnsupportedFilesystemKind,
        detail: String,
    },
    /// The notifier reported that it dropped events; the window has a hole of unknown size.
    EventQueueOverflow { detail: String },
    /// The notifier reported a hard error after arming.
    BackendFailed { detail: String },
    /// One window accumulated more distinct paths than [`FILESYSTEM_OBSERVER_DIRTY_PATH_CAP`].
    ObservationBudgetExhausted { limit: usize },
    /// Another window over the same session overlapped this one, so neither owns its accounting.
    ConcurrentObservation,
}

impl FilesystemObserverGap {
    /// Stable machine-readable identity for gap text and typed output fields.
    pub fn id(&self) -> &'static str {
        match self {
            Self::BackendUnavailable { .. } => "observer_backend_unavailable",
            Self::UnsupportedFilesystem { .. } => "observer_unsupported_filesystem",
            Self::EventQueueOverflow { .. } => "observer_event_queue_overflow",
            Self::BackendFailed { .. } => "observer_backend_failed",
            Self::ObservationBudgetExhausted { .. } => "observer_budget_exhausted",
            Self::ConcurrentObservation => "observer_concurrent_observation",
        }
    }

    /// Operator-facing detail. Never used for control flow.
    pub fn detail(&self) -> String {
        match self {
            Self::BackendUnavailable { detail } | Self::BackendFailed { detail } => detail.clone(),
            Self::UnsupportedFilesystem { kind, detail } => {
                format!("{} ({detail})", kind.id())
            }
            Self::EventQueueOverflow { detail } => detail.clone(),
            Self::ObservationBudgetExhausted { limit } => {
                format!("observation exceeded {limit} distinct paths")
            }
            Self::ConcurrentObservation => {
                "another observation window overlapped this one".to_string()
            }
        }
    }
}

/// Which observed paths one session accounts for at all.
///
/// The filter runs *before* anything is charged: an ignored path advances no epoch, occupies no
/// accumulator slot, and consumes no budget. That ordering is the whole point. A recursive watch
/// cannot be narrowed at the kernel, so the only place churn can be dropped is on arrival, and
/// dropping it after the budget has already been spent would let a single `cargo build` overlapping
/// one scan exhaust a 4096-path window on paths the escalation logic discards anyway.
///
/// Dropping is safe in exactly one direction: a path this filter refuses can never be admitted
/// source, so refusing it can only ever *lower* the number of mutations a window reports. It can
/// never manufacture a mutation, and it can never hide one a caller could have acted on.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ObservedScopeFilter {
    ignored_directory_names: BTreeSet<String>,
    ignored_roots: BTreeSet<PathBuf>,
}

impl ObservedScopeFilter {
    /// Account for every path under the watched root.
    pub fn observe_everything() -> Self {
        Self::default()
    }

    /// Account for everything discovery could admit, and nothing else.
    pub fn source_default() -> Self {
        Self {
            ignored_directory_names: OBSERVER_IGNORED_DIRECTORY_NAMES
                .iter()
                .map(|name| (*name).to_string())
                .collect(),
            ignored_roots: BTreeSet::new(),
        }
    }

    /// Also ignore one absolute subtree, such as the storage-owned cache directory.
    #[must_use]
    pub fn ignoring_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.ignored_roots.insert(root.into());
        self
    }

    /// Whether `path` under `root` can possibly name something discovery admitted.
    pub fn admits(&self, root: &Path, path: &Path) -> bool {
        let Ok(relative) = path.strip_prefix(root) else {
            // A recursive watch reports nothing outside its root; anything that arrives anyway
            // names no file inside the project and can carry no verdict.
            return false;
        };
        if self
            .ignored_roots
            .iter()
            .any(|ignored| path.starts_with(ignored))
        {
            return false;
        }
        !relative.components().any(|component| {
            component
                .as_os_str()
                .to_str()
                .is_some_and(|name| self.ignored_directory_names.contains(name))
        })
    }
}

/// What a mutation implies about the scope a caller must re-examine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MutationScope {
    /// One file's bytes or metadata changed. Rehashing that path settles it.
    File,
    /// A directory's membership changed. Only rescanning that scope settles it.
    Directory,
}

/// One mutation, or one loss of coverage, as reported by a notifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObservedFilesystemEvent {
    /// A path changed.
    Mutated { path: PathBuf, scope: MutationScope },
    /// The notifier dropped events it cannot describe.
    CoverageLost { detail: String },
    /// The notifier failed after arming.
    Failed { detail: String },
}

/// A window the observer covered end to end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProvenCoverage {
    identity: FilesystemObserverIdentity,
    armed_epoch: ObserverEpoch,
    sealed_epoch: ObserverEpoch,
    dirty_paths: BTreeSet<PathBuf>,
    rescan_roots: BTreeSet<PathBuf>,
}

impl ProvenCoverage {
    /// Identity of the session that covered the window.
    pub fn identity(&self) -> &FilesystemObserverIdentity {
        &self.identity
    }

    /// Mutation count at the moment the window opened, before the caller's scan ran.
    pub fn armed_epoch(&self) -> ObserverEpoch {
        self.armed_epoch
    }

    /// Mutation count at the moment the window closed.
    pub fn sealed_epoch(&self) -> ObserverEpoch {
        self.sealed_epoch
    }

    /// No mutation the scope filter admits was observed while the caller's scan ran.
    ///
    /// Churn the filter refuses — `target/`, `.git/`, the storage cache — never counted, so a
    /// build running alongside a scan still leaves the window quiet.
    pub fn is_quiet(&self) -> bool {
        self.armed_epoch == self.sealed_epoch
    }

    /// Files whose bytes or metadata changed during the window. Rehash these.
    pub fn dirty_paths(&self) -> &BTreeSet<PathBuf> {
        &self.dirty_paths
    }

    /// Directories whose membership changed during the window. Rescan these scopes.
    pub fn rescan_roots(&self) -> &BTreeSet<PathBuf> {
        &self.rescan_roots
    }
}

/// The result of one observation window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilesystemObserverCoverage {
    /// The observer accounted for the whole window and names every mutation inside it.
    Proven(ProvenCoverage),
    /// Nothing may be concluded from this window.
    Indeterminate { gap: FilesystemObserverGap },
}

impl FilesystemObserverCoverage {
    /// The coverage that was proven, or `None` when the window sealed indeterminate.
    pub fn proven(&self) -> Option<&ProvenCoverage> {
        match self {
            Self::Proven(coverage) => Some(coverage),
            Self::Indeterminate { .. } => None,
        }
    }

    /// The gap, when the window sealed indeterminate.
    pub fn gap(&self) -> Option<&FilesystemObserverGap> {
        match self {
            Self::Proven(_) => None,
            Self::Indeterminate { gap } => Some(gap),
        }
    }
}

/// A source of filesystem events for one armed session.
///
/// Implementors must report every loss they know about; a source that silently swallows a
/// dropped event turns an indeterminate window into a false proof, which is the one failure this
/// module cannot tolerate.
pub trait ObserverEventSource: Send {
    /// Take everything queued since the previous call. Must not block.
    fn drain(&mut self) -> Vec<ObservedFilesystemEvent>;
}

struct SessionState {
    epoch: ObserverEpoch,
    dirty: BTreeSet<PathBuf>,
    rescan: BTreeSet<PathBuf>,
    /// Losses the session can never come back from. A backend that dropped one event cannot say
    /// what it dropped, and on inotify the descriptors for subtrees created during the overflow
    /// were never installed, so nothing later re-establishes coverage.
    gap: Option<FilesystemObserverGap>,
    /// The current window overflowed its own accumulator. Cleared when every window closes,
    /// because the accumulator is per-window and the next caller re-reads the tree regardless.
    budget_exhausted: bool,
    /// How many windows this session has overflowed. Diagnostic only, never control flow.
    budget_exhaustions: u64,
    open_windows: usize,
    contended: bool,
    source: Box<dyn ObserverEventSource>,
}

/// One armed observation of one root.
///
/// Arm it, run the authoritative scan inside [`FilesystemObserverSession::observe_window`], and
/// read the sealed coverage. There is deliberately no way to seal a window that was not opened
/// before the scan: the ordering the whole design depends on is enforced by the call shape rather
/// than by a comment.
pub struct FilesystemObserverSession {
    identity: FilesystemObserverIdentity,
    canonical_root: PathBuf,
    scope: ObservedScopeFilter,
    state: Mutex<SessionState>,
}

impl std::fmt::Debug for FilesystemObserverSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FilesystemObserverSession")
            .field("identity", &self.identity)
            .finish_non_exhaustive()
    }
}

impl FilesystemObserverSession {
    /// Arm the platform notifier over `root`.
    ///
    /// Fails closed with a typed gap when the filesystem is unsupported or the notifier refuses.
    pub fn arm(root: &Path) -> Result<Self, FilesystemObserverGap> {
        Self::arm_with_scope(root, ObservedScopeFilter::source_default())
    }

    /// Arm the platform notifier over `root`, accounting only for paths `scope` admits.
    pub fn arm_with_scope(
        root: &Path,
        scope: ObservedScopeFilter,
    ) -> Result<Self, FilesystemObserverGap> {
        Self::arm_with_filesystem_and_scope(root, &probe_host_filesystem(root), scope)
    }

    /// Arm the platform notifier over `root` against a filesystem description the caller already
    /// has.
    ///
    /// The support check runs first and refuses before any notifier is created, so an unsupported
    /// root costs nothing and can never end up half-armed.
    pub fn arm_with_filesystem(
        root: &Path,
        observed: &ObservedFilesystem,
    ) -> Result<Self, FilesystemObserverGap> {
        Self::arm_with_filesystem_and_scope(root, observed, ObservedScopeFilter::source_default())
    }

    /// Arm against a caller-supplied filesystem description and scope filter.
    pub fn arm_with_filesystem_and_scope(
        root: &Path,
        observed: &ObservedFilesystem,
        scope: ObservedScopeFilter,
    ) -> Result<Self, FilesystemObserverGap> {
        classify_observed_filesystem(root, observed)?;
        let source = native_event_source(root)?;
        Ok(Self::from_source(
            FilesystemObserverBackend::Native,
            root,
            Box::new(source),
            scope,
        ))
    }

    /// Arm a session over a caller-supplied event source.
    ///
    /// Test-only: production always arms the platform notifier through [`Self::arm`].
    #[cfg(any(test, feature = "test-support"))]
    pub fn arm_with_source(root: &Path, source: Box<dyn ObserverEventSource>) -> Self {
        Self::from_source(
            FilesystemObserverBackend::Injected,
            root,
            source,
            ObservedScopeFilter::source_default(),
        )
    }

    /// Arm a session over a caller-supplied event source and scope filter.
    #[cfg(any(test, feature = "test-support"))]
    pub fn arm_with_source_and_scope(
        root: &Path,
        source: Box<dyn ObserverEventSource>,
        scope: ObservedScopeFilter,
    ) -> Self {
        Self::from_source(FilesystemObserverBackend::Injected, root, source, scope)
    }

    fn from_source(
        backend: FilesystemObserverBackend,
        root: &Path,
        source: Box<dyn ObserverEventSource>,
        scope: ObservedScopeFilter,
    ) -> Self {
        let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        Self {
            identity: FilesystemObserverIdentity {
                backend,
                root: root.to_path_buf(),
                session_id: Uuid::new_v4().to_string(),
            },
            canonical_root,
            scope,
            state: Mutex::new(SessionState {
                epoch: 0,
                dirty: BTreeSet::new(),
                rescan: BTreeSet::new(),
                gap: None,
                budget_exhausted: false,
                budget_exhaustions: 0,
                open_windows: 0,
                contended: false,
                source,
            }),
        }
    }

    /// Identity of this armed session.
    pub fn identity(&self) -> &FilesystemObserverIdentity {
        &self.identity
    }

    /// Which observed paths this session accounts for.
    pub fn scope(&self) -> &ObservedScopeFilter {
        &self.scope
    }

    /// How many windows this session has overflowed its per-window path budget.
    ///
    /// Exhaustion is otherwise invisible from outside — the symptom is that reads keep returning
    /// whatever the unobserved scan said — so an operator asking "why is this repository never
    /// observed" needs a number to look at.
    pub fn budget_exhaustion_count(&self) -> u64 {
        self.lock().budget_exhaustions
    }

    /// Mutations absorbed so far, after taking whatever the notifier has queued.
    ///
    /// Reading the epoch outside a window is only meaningful for diagnostics; a freshness claim
    /// must come from a sealed window.
    pub fn epoch(&self) -> ObserverEpoch {
        let mut state = self.lock();
        self.absorb(&mut state);
        state.epoch
    }

    /// The sticky gap this session has fallen into, if any.
    ///
    /// Per-window losses — an overflowed accumulator, an overlapping window — are not sticky and
    /// never appear here; they are reported by the window that suffered them and nowhere else.
    pub fn gap(&self) -> Option<FilesystemObserverGap> {
        let mut state = self.lock();
        self.absorb(&mut state);
        state.gap.clone()
    }

    /// Run `scan` inside an observation window and report what the observer saw while it ran.
    ///
    /// Everything queued before the window is absorbed and discarded first, so the sealed
    /// coverage describes the scan's window and nothing else. `scan` always runs, including when
    /// the session is already indeterminate — the observer never decides whether the caller's own
    /// work happens.
    pub fn observe_window<T>(&self, scan: impl FnOnce() -> T) -> (T, FilesystemObserverCoverage) {
        let armed_epoch = {
            let mut state = self.lock();
            self.absorb(&mut state);
            if state.open_windows == 0 {
                state.dirty.clear();
                state.rescan.clear();
                state.budget_exhausted = false;
            } else {
                // Two windows sharing one accumulator can each attribute the other's quiet
                // stretches to itself. Neither may claim proof.
                state.contended = true;
            }
            state.open_windows += 1;
            state.epoch
        };
        let value = scan();
        let coverage = {
            let mut state = self.lock();
            self.absorb(&mut state);
            state.open_windows = state.open_windows.saturating_sub(1);
            let contended = state.contended;
            let exhausted = state.budget_exhausted;
            if state.open_windows == 0 {
                state.contended = false;
                state.budget_exhausted = false;
            }
            match (contended, exhausted, state.gap.clone()) {
                (_, _, Some(gap)) => FilesystemObserverCoverage::Indeterminate { gap },
                (true, _, None) => FilesystemObserverCoverage::Indeterminate {
                    gap: FilesystemObserverGap::ConcurrentObservation,
                },
                (false, true, None) => FilesystemObserverCoverage::Indeterminate {
                    gap: FilesystemObserverGap::ObservationBudgetExhausted {
                        limit: FILESYSTEM_OBSERVER_DIRTY_PATH_CAP,
                    },
                },
                (false, false, None) => FilesystemObserverCoverage::Proven(ProvenCoverage {
                    identity: self.identity.clone(),
                    armed_epoch,
                    sealed_epoch: state.epoch,
                    dirty_paths: state.dirty.clone(),
                    rescan_roots: state.rescan.clone(),
                }),
            }
        };
        (value, coverage)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, SessionState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn absorb(&self, state: &mut SessionState) {
        for event in state.source.drain() {
            self.apply(state, event);
        }
    }

    fn apply(&self, state: &mut SessionState, event: ObservedFilesystemEvent) {
        match event {
            ObservedFilesystemEvent::Mutated { path, scope } => {
                let path = self.rebase_observed_path(path);
                // Admission first, budget second. A path no caller can act on must not advance
                // the epoch, must not occupy an accumulator slot, and must not spend the window.
                if !self.scope.admits(&self.identity.root, &path) {
                    return;
                }
                state.epoch = state.epoch.saturating_add(1);
                let set = match scope {
                    MutationScope::File => &mut state.dirty,
                    MutationScope::Directory => &mut state.rescan,
                };
                if set.len() >= FILESYSTEM_OBSERVER_DIRTY_PATH_CAP && !set.contains(&path) {
                    if !state.budget_exhausted {
                        state.budget_exhausted = true;
                        state.budget_exhaustions = state.budget_exhaustions.saturating_add(1);
                    }
                    return;
                }
                set.insert(path);
            }
            ObservedFilesystemEvent::CoverageLost { detail } => {
                state
                    .gap
                    .get_or_insert(FilesystemObserverGap::EventQueueOverflow { detail });
            }
            ObservedFilesystemEvent::Failed { detail } => {
                state
                    .gap
                    .get_or_insert(FilesystemObserverGap::BackendFailed { detail });
            }
        }
    }

    /// Re-root an observed path onto the spelling the caller armed with.
    ///
    /// Platform notifiers report resolved paths. On hosts where the watched root reaches the
    /// volume through a symlink — `/tmp` and `/var` on macOS are the everyday case — every
    /// observed path would otherwise fail to line up with the paths the caller's scan produced,
    /// and every mutation would be silently discarded as out of scope.
    fn rebase_observed_path(&self, path: PathBuf) -> PathBuf {
        if self.canonical_root == self.identity.root {
            return path;
        }
        match path.strip_prefix(&self.canonical_root) {
            Ok(suffix) => self.identity.root.join(suffix),
            Err(_) => path,
        }
    }
}

/// Map one notifier event onto the mutations a caller must account for.
///
/// A dropped-event flag outranks everything else in the same event: the notifier is telling the
/// caller that it cannot describe what happened, and no path list alongside it makes the window
/// whole again.
///
/// Anything the notifier could not classify is treated as a directory-scoped mutation rather
/// than a file-scoped one, because rescanning a scope is the answer that stays correct when the
/// event turns out to have been a create, a rename, or a removal.
pub fn classify_notify_event(event: &notify::Event) -> Vec<ObservedFilesystemEvent> {
    use notify::event::{CreateKind, EventKind, ModifyKind, RemoveKind};

    if event.need_rescan() {
        return vec![ObservedFilesystemEvent::CoverageLost {
            detail: "platform notifier reported dropped events".to_string(),
        }];
    }

    let mut observed = Vec::new();
    for path in &event.paths {
        match event.kind {
            // Opening, reading, or closing a file is not a mutation.
            EventKind::Access(_) => {}
            EventKind::Create(CreateKind::Folder) | EventKind::Remove(RemoveKind::Folder) => {
                observed.push(ObservedFilesystemEvent::Mutated {
                    path: path.clone(),
                    scope: MutationScope::Directory,
                });
                observed.push(ObservedFilesystemEvent::Mutated {
                    path: parent_or_self(path),
                    scope: MutationScope::Directory,
                });
            }
            EventKind::Create(_)
            | EventKind::Remove(_)
            | EventKind::Modify(ModifyKind::Name(_)) => {
                // The directory listing changed, and a scan that already passed this directory
                // cannot know whether it saw the entry before or after.
                observed.push(ObservedFilesystemEvent::Mutated {
                    path: parent_or_self(path),
                    scope: MutationScope::Directory,
                });
            }
            EventKind::Modify(_) => observed.push(ObservedFilesystemEvent::Mutated {
                path: path.clone(),
                scope: MutationScope::File,
            }),
            EventKind::Any | EventKind::Other => {
                observed.push(ObservedFilesystemEvent::Mutated {
                    path: parent_or_self(path),
                    scope: MutationScope::Directory,
                });
            }
        }
    }
    observed
}

fn parent_or_self(path: &Path) -> PathBuf {
    path.parent()
        .map_or_else(|| path.to_path_buf(), Path::to_path_buf)
}

struct NativeEventSource {
    _watcher: notify::RecommendedWatcher,
    events: Receiver<Result<notify::Event, notify::Error>>,
}

impl ObserverEventSource for NativeEventSource {
    fn drain(&mut self) -> Vec<ObservedFilesystemEvent> {
        let mut observed = Vec::new();
        loop {
            match self.events.try_recv() {
                Ok(Ok(event)) => observed.extend(classify_notify_event(&event)),
                Ok(Err(error)) => observed.push(ObservedFilesystemEvent::Failed {
                    detail: format!("platform notifier error: {error}"),
                }),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    observed.push(ObservedFilesystemEvent::Failed {
                        detail: "platform notifier channel closed".to_string(),
                    });
                    break;
                }
            }
        }
        observed
    }
}

fn native_event_source(root: &Path) -> Result<NativeEventSource, FilesystemObserverGap> {
    use notify::{RecursiveMode, Watcher};

    let (sender, events) = std::sync::mpsc::channel();
    let mut watcher = notify::recommended_watcher(move |event| {
        let _ = sender.send(event);
    })
    .map_err(|error| FilesystemObserverGap::BackendUnavailable {
        detail: format!("failed to create platform notifier: {error}"),
    })?;
    watcher
        .watch(root, RecursiveMode::Recursive)
        .map_err(|error| FilesystemObserverGap::BackendUnavailable {
            detail: format!("failed to watch {}: {error}", root.display()),
        })?;
    Ok(NativeEventSource {
        _watcher: watcher,
        events,
    })
}

/// Classify a filesystem type name against the notification support CodeStory requires.
///
/// The name is whatever the host calls the filesystem: `f_fstypename` on macOS, the mount-table
/// type on Linux. Suffixed names (`fuse.sshfs`) are classified by their suffix first, so a
/// network filesystem tunnelled through FUSE is recognised as remote rather than merely
/// userspace.
pub fn classify_filesystem_type_name(name: &str) -> Option<UnsupportedFilesystemKind> {
    const NETWORK: &[&str] = &[
        "nfs",
        "nfs3",
        "nfs4",
        "cifs",
        "smb",
        "smb2",
        "smb3",
        "smbfs",
        "afpfs",
        "afp",
        "afs",
        "ncpfs",
        "glusterfs",
        "ceph",
        "webdav",
        "davfs",
        "davfs2",
        "sshfs",
        "ftpfs",
        "curlftpfs",
        "s3fs",
    ];
    const WSL_INTEROP: &[&str] = &["9p", "v9fs", "drvfs", "drvfs2", "lxfs", "plan9"];

    let name = name.trim().to_ascii_lowercase();
    if name.is_empty() {
        return None;
    }
    let suffix = name.rsplit('.').next().unwrap_or(name.as_str());
    for candidate in [name.as_str(), suffix] {
        if NETWORK.contains(&candidate) {
            return Some(UnsupportedFilesystemKind::NetworkMount);
        }
        if WSL_INTEROP.contains(&candidate) {
            return Some(UnsupportedFilesystemKind::WindowsSubsystemForLinuxInterop);
        }
    }
    // `macfuse`, `osxfuse`, `fuse`, `fuseblk`, `fuse.<driver>`: the driver, not the kernel,
    // decides what notification these deliver, so no claim may rest on them.
    if name == "fuseblk" || name.ends_with("fuse") || name.starts_with("fuse.") {
        return Some(UnsupportedFilesystemKind::UserspaceMount);
    }
    None
}

/// Classify the mount that owns `root` from a Linux `/proc/self/mountinfo` table.
///
/// The deepest mount point that contains the root wins, because a repository under an NFS mount
/// nested inside a local filesystem is on NFS.
pub fn classify_mount_table(
    root: &Path,
    mountinfo: &str,
) -> Option<(UnsupportedFilesystemKind, String)> {
    let mut best: Option<(usize, String)> = None;
    for line in mountinfo.lines() {
        let Some((before, after)) = line.split_once(" - ") else {
            continue;
        };
        let mount_point = before.split_whitespace().nth(4)?;
        let mount_point = decode_mountinfo_field(mount_point);
        let Some(filesystem_type) = after.split_whitespace().next() else {
            continue;
        };
        if !root.starts_with(&mount_point) {
            continue;
        }
        let depth = Path::new(&mount_point).components().count();
        if best.as_ref().is_none_or(|(best, _)| depth >= *best) {
            best = Some((depth, filesystem_type.to_string()));
        }
    }
    let (_, filesystem_type) = best?;
    classify_filesystem_type_name(&filesystem_type).map(|kind| (kind, filesystem_type))
}

fn decode_mountinfo_field(field: &str) -> String {
    let mut decoded = String::with_capacity(field.len());
    let mut characters = field.chars();
    while let Some(character) = characters.next() {
        if character != '\\' {
            decoded.push(character);
            continue;
        }
        let escape = characters.by_ref().take(3).collect::<String>();
        match u8::from_str_radix(&escape, 8) {
            Ok(value) => decoded.push(char::from(value)),
            Err(_) => {
                decoded.push('\\');
                decoded.push_str(&escape);
            }
        }
    }
    decoded
}

/// Classify a Linux root reached through WSL's Windows-drive interop.
///
/// The mount table already catches WSL 2, whose drive mounts are 9p. WSL 1 presents its own
/// filesystem driver under names the table may not carry, so the kernel release string plus the
/// `/mnt/<drive>` shape stands in for it.
pub fn classify_wsl_interop(
    os_release: Option<&str>,
    root: &Path,
) -> Option<UnsupportedFilesystemKind> {
    let os_release = os_release?.to_ascii_lowercase();
    if !os_release.contains("microsoft") && !os_release.contains("wsl") {
        return None;
    }
    let mut components = root.components();
    if components.next() != Some(std::path::Component::RootDir) {
        return None;
    }
    if components.next()?.as_os_str() != "mnt" {
        return None;
    }
    let drive = components.next()?.as_os_str().to_str()?;
    let mut characters = drive.chars();
    let letter = characters.next()?;
    if characters.next().is_some() || !letter.is_ascii_alphabetic() {
        return None;
    }
    Some(UnsupportedFilesystemKind::WindowsSubsystemForLinuxInterop)
}

/// Classify a Windows root by path shape alone.
///
/// UNC roots name a server, and `\\wsl$\` and `\\wsl.localhost\` reach a Linux filesystem through
/// a translation layer. Neither delivers change notification a freshness claim can rest on. The
/// `\\?\` and `\\.\` prefixes are local device namespaces and are left to the drive checks.
pub fn classify_windows_path_shape(root: &Path) -> Option<UnsupportedFilesystemKind> {
    let spelling = root.to_string_lossy().to_ascii_lowercase();
    let spelling = spelling.replace('/', "\\");
    if spelling.starts_with("\\\\wsl$\\") || spelling.starts_with("\\\\wsl.localhost\\") {
        return Some(UnsupportedFilesystemKind::WindowsSubsystemForLinuxInterop);
    }
    if spelling.starts_with("\\\\?\\unc\\") {
        return Some(UnsupportedFilesystemKind::NetworkMount);
    }
    if spelling.starts_with("\\\\")
        && !spelling.starts_with("\\\\?\\")
        && !spelling.starts_with("\\\\.\\")
    {
        return Some(UnsupportedFilesystemKind::NetworkMount);
    }
    None
}

fn unsupported(kind: UnsupportedFilesystemKind, detail: String) -> FilesystemObserverGap {
    FilesystemObserverGap::UnsupportedFilesystem { kind, detail }
}

/// Whatever this host can say about the filesystem under a root.
///
/// Platforms disagree about which of these they expose — macOS names the filesystem directly,
/// Linux describes it through the mount table and kernel release, Windows says nothing beyond the
/// path shape — so every field is optional and the classification simply uses what it is given.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ObservedFilesystem {
    /// The host's name for the filesystem under the root.
    pub type_name: Option<String>,
    /// The kernel release string.
    pub os_release: Option<String>,
    /// A Linux `/proc/self/mountinfo` table.
    pub mount_table: Option<String>,
}

/// Decide whether a root's filesystem can carry a freshness claim.
///
/// Every signal is checked, not just the first one the platform happens to provide: a WSL 2 drive
/// mount is both a `9p` mount-table entry and a `/mnt/<drive>` path under a Microsoft kernel, and
/// a host that reports only one of those must still be refused.
pub fn classify_observed_filesystem(
    root: &Path,
    observed: &ObservedFilesystem,
) -> Result<(), FilesystemObserverGap> {
    if let Some(kind) = classify_windows_path_shape(root) {
        return Err(unsupported(kind, "unsupported windows root".to_string()));
    }
    if let Some(kind) = classify_wsl_interop(observed.os_release.as_deref(), root) {
        return Err(unsupported(kind, "windows drive interop mount".to_string()));
    }
    if let Some(type_name) = observed.type_name.as_deref()
        && let Some(kind) = classify_filesystem_type_name(type_name)
    {
        return Err(unsupported(kind, type_name.to_string()));
    }
    if let Some(mount_table) = observed.mount_table.as_deref()
        && let Some((kind, type_name)) = classify_mount_table(root, mount_table)
    {
        return Err(unsupported(kind, type_name));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn probe_host_filesystem(_root: &Path) -> ObservedFilesystem {
    ObservedFilesystem {
        type_name: None,
        os_release: std::fs::read_to_string("/proc/sys/kernel/osrelease").ok(),
        mount_table: std::fs::read_to_string("/proc/self/mountinfo").ok(),
    }
}

#[cfg(target_os = "macos")]
fn probe_host_filesystem(root: &Path) -> ObservedFilesystem {
    ObservedFilesystem {
        type_name: observed_filesystem_type_name(root),
        os_release: None,
        mount_table: None,
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn probe_host_filesystem(_root: &Path) -> ObservedFilesystem {
    // Windows is fully covered by the path-shape rules, which run for every host.
    ObservedFilesystem::default()
}

#[cfg(target_os = "macos")]
fn observed_filesystem_type_name(root: &Path) -> Option<String> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let path = CString::new(root.as_os_str().as_bytes()).ok()?;
    // SAFETY: `path` is a NUL-terminated C string that outlives the call, and `statfs` only
    // writes into the zeroed output record it is handed.
    let mut record: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(path.as_ptr(), &mut record) } != 0 {
        return None;
    }
    let bytes = record
        .f_fstypename
        .iter()
        .take_while(|byte| **byte != 0)
        .map(|byte| *byte as u8)
        .collect::<Vec<_>>();
    String::from_utf8(bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{
        AccessKind, CreateKind, DataChange, EventKind, ModifyKind, RemoveKind, RenameMode,
    };
    use std::sync::mpsc::{Sender, channel};
    use std::time::{Duration, Instant};

    struct ScriptedSource {
        events: Receiver<ObservedFilesystemEvent>,
    }

    impl ObserverEventSource for ScriptedSource {
        fn drain(&mut self) -> Vec<ObservedFilesystemEvent> {
            let mut observed = Vec::new();
            while let Ok(event) = self.events.try_recv() {
                observed.push(event);
            }
            observed
        }
    }

    fn scripted(root: &Path) -> (FilesystemObserverSession, Sender<ObservedFilesystemEvent>) {
        let (sender, events) = channel();
        let session =
            FilesystemObserverSession::arm_with_source(root, Box::new(ScriptedSource { events }));
        (session, sender)
    }

    fn mutated(path: &str, scope: MutationScope) -> ObservedFilesystemEvent {
        ObservedFilesystemEvent::Mutated {
            path: PathBuf::from(path),
            scope,
        }
    }

    #[test]
    fn a_quiet_window_seals_proven_and_names_no_drift() {
        let (session, _sender) = scripted(Path::new("/repo"));
        let (value, coverage) = session.observe_window(|| 7);
        assert_eq!(value, 7);
        let proven = coverage.proven().expect("a quiet window must seal proven");
        assert!(proven.is_quiet());
        assert_eq!(proven.armed_epoch(), proven.sealed_epoch());
        assert!(proven.dirty_paths().is_empty());
        assert!(proven.rescan_roots().is_empty());
        assert_eq!(proven.identity(), session.identity());
    }

    #[test]
    fn a_mutation_during_the_window_is_named_and_advances_the_epoch() {
        let (session, sender) = scripted(Path::new("/repo"));
        let (_, coverage) = session.observe_window(|| {
            sender
                .send(mutated("/repo/src/lib.rs", MutationScope::File))
                .expect("scripted source must accept the event");
            sender
                .send(mutated("/repo/src", MutationScope::Directory))
                .expect("scripted source must accept the event");
        });
        let proven = coverage.proven().expect("no loss was reported");
        assert!(!proven.is_quiet());
        assert_eq!(proven.sealed_epoch(), proven.armed_epoch() + 2);
        assert_eq!(
            proven.dirty_paths().iter().collect::<Vec<_>>(),
            vec![&PathBuf::from("/repo/src/lib.rs")]
        );
        assert_eq!(
            proven.rescan_roots().iter().collect::<Vec<_>>(),
            vec![&PathBuf::from("/repo/src")]
        );
    }

    #[test]
    fn mutations_before_the_window_never_enter_it() {
        let (session, sender) = scripted(Path::new("/repo"));
        sender
            .send(mutated("/repo/src/before.rs", MutationScope::File))
            .expect("scripted source must accept the event");
        let (_, coverage) = session.observe_window(|| {});
        let proven = coverage.proven().expect("no loss was reported");
        assert!(
            proven.is_quiet(),
            "a pre-window mutation must be absorbed before the window opens"
        );
        assert!(proven.dirty_paths().is_empty());
        assert!(
            proven.armed_epoch() > 0,
            "the absorbed mutation must still advance the session epoch"
        );
    }

    #[test]
    fn a_path_dirtied_before_and_again_inside_the_window_is_still_reported() {
        // Accumulating across windows and diffing the sets would hide exactly this case.
        let (session, sender) = scripted(Path::new("/repo"));
        sender
            .send(mutated("/repo/src/lib.rs", MutationScope::File))
            .expect("scripted source must accept the event");
        let (_, coverage) = session.observe_window(|| {
            sender
                .send(mutated("/repo/src/lib.rs", MutationScope::File))
                .expect("scripted source must accept the event");
        });
        let proven = coverage.proven().expect("no loss was reported");
        assert_eq!(
            proven.dirty_paths().iter().collect::<Vec<_>>(),
            vec![&PathBuf::from("/repo/src/lib.rs")],
            "the second mutation belongs to this window"
        );
        assert!(!proven.is_quiet());
    }

    #[test]
    fn a_dropped_event_seals_indeterminate_and_stays_that_way() {
        let (session, sender) = scripted(Path::new("/repo"));
        let (_, coverage) = session.observe_window(|| {
            sender
                .send(ObservedFilesystemEvent::CoverageLost {
                    detail: "queue overflow".to_string(),
                })
                .expect("scripted source must accept the event");
        });
        assert_eq!(
            coverage.gap().map(FilesystemObserverGap::id),
            Some("observer_event_queue_overflow")
        );

        let (_, later) = session.observe_window(|| {});
        assert!(
            later.proven().is_none(),
            "a session that lost coverage can never prove a later window"
        );
        assert_eq!(
            later.gap().map(FilesystemObserverGap::id),
            Some("observer_event_queue_overflow")
        );
    }

    #[test]
    fn a_backend_failure_seals_indeterminate() {
        let (session, sender) = scripted(Path::new("/repo"));
        let (_, coverage) = session.observe_window(|| {
            sender
                .send(ObservedFilesystemEvent::Failed {
                    detail: "watch descriptor exhausted".to_string(),
                })
                .expect("scripted source must accept the event");
        });
        assert_eq!(
            coverage.gap().map(FilesystemObserverGap::id),
            Some("observer_backend_failed")
        );
    }

    #[test]
    fn exhausting_the_path_budget_seals_indeterminate() {
        let (session, sender) = scripted(Path::new("/repo"));
        let (_, coverage) = session.observe_window(|| {
            for index in 0..=FILESYSTEM_OBSERVER_DIRTY_PATH_CAP {
                sender
                    .send(mutated(
                        &format!("/repo/src/file{index}.rs"),
                        MutationScope::File,
                    ))
                    .expect("scripted source must accept the event");
            }
        });
        assert_eq!(
            coverage.gap().map(FilesystemObserverGap::id),
            Some("observer_budget_exhausted")
        );
    }

    #[test]
    fn an_exhausted_budget_is_a_per_window_loss_the_next_window_recovers_from() {
        // The permanently-disabled observer is the failure this guards: a single build overlapping
        // one scan must not silently switch the feature off for the rest of the process.
        let (session, sender) = scripted(Path::new("/repo"));
        let (_, exhausted) = session.observe_window(|| {
            for index in 0..=FILESYSTEM_OBSERVER_DIRTY_PATH_CAP {
                sender
                    .send(mutated(
                        &format!("/repo/src/file{index}.rs"),
                        MutationScope::File,
                    ))
                    .expect("scripted source must accept the event");
            }
        });
        assert_eq!(
            exhausted.gap().map(FilesystemObserverGap::id),
            Some("observer_budget_exhausted")
        );
        assert_eq!(session.budget_exhaustion_count(), 1);
        assert_eq!(
            session.gap(),
            None,
            "overflowing our own accumulator is not a backend loss and must not stick"
        );

        let (_, recovered) = session.observe_window(|| {
            sender
                .send(mutated("/repo/src/lib.rs", MutationScope::File))
                .expect("scripted source must accept the event");
        });
        let proven = recovered
            .proven()
            .expect("the window after an overflow accounts for itself completely");
        assert_eq!(
            proven.dirty_paths().iter().collect::<Vec<_>>(),
            vec![&PathBuf::from("/repo/src/lib.rs")]
        );
        assert_eq!(session.budget_exhaustion_count(), 1);
    }

    #[test]
    fn ignored_churn_never_reaches_the_epoch_or_the_budget() {
        // A build and a checkout produce more paths than the whole budget. Charging them before
        // the admission filter would seal every overlapping window indeterminate.
        let (session, sender) = scripted(Path::new("/repo"));
        let (_, coverage) = session.observe_window(|| {
            for index in 0..=FILESYSTEM_OBSERVER_DIRTY_PATH_CAP {
                sender
                    .send(mutated(
                        &format!("/repo/target/debug/artifact{index}.o"),
                        MutationScope::File,
                    ))
                    .expect("scripted source must accept the event");
                sender
                    .send(mutated(
                        &format!("/repo/.git/objects/{index}"),
                        MutationScope::File,
                    ))
                    .expect("scripted source must accept the event");
                sender
                    .send(mutated(
                        &format!("/repo/node_modules/dep{index}/index.js"),
                        MutationScope::File,
                    ))
                    .expect("scripted source must accept the event");
            }
            sender
                .send(mutated("/repo/src/lib.rs", MutationScope::File))
                .expect("scripted source must accept the event");
        });
        let proven = coverage
            .proven()
            .expect("churn discovery excludes may not exhaust the observation budget");
        assert_eq!(session.budget_exhaustion_count(), 0);
        assert_eq!(
            proven.dirty_paths().iter().collect::<Vec<_>>(),
            vec![&PathBuf::from("/repo/src/lib.rs")],
            "only paths discovery could admit are accounted for"
        );
        assert_eq!(
            proven.sealed_epoch(),
            proven.armed_epoch() + 1,
            "ignored churn advances no epoch, so a lease cannot be invalidated by a build"
        );
    }

    #[test]
    fn an_ignored_root_covers_the_storage_cache_the_runtime_owns() {
        let scope = ObservedScopeFilter::source_default().ignoring_root("/repo/.cache");
        assert!(scope.admits(Path::new("/repo"), Path::new("/repo/src/lib.rs")));
        assert!(scope.admits(Path::new("/repo"), Path::new("/repo")));
        assert!(!scope.admits(Path::new("/repo"), Path::new("/repo/.cache/codestory.db")));
        assert!(!scope.admits(Path::new("/repo"), Path::new("/repo/target/debug/main")));
        assert!(!scope.admits(Path::new("/repo"), Path::new("/repo/a/.git/index")));
        assert!(
            !scope.admits(Path::new("/repo"), Path::new("/elsewhere/lib.rs")),
            "a recursive watch reports nothing outside its root; anything that arrives is noise"
        );
        assert!(
            ObservedScopeFilter::observe_everything()
                .admits(Path::new("/repo"), Path::new("/repo/target/debug/main"))
        );
    }

    #[test]
    fn overlapping_windows_both_seal_indeterminate() {
        let (session, sender) = scripted(Path::new("/repo"));
        let mut inner = None;
        let (_, outer) = session.observe_window(|| {
            sender
                .send(mutated("/repo/src/lib.rs", MutationScope::File))
                .expect("scripted source must accept the event");
            let (_, coverage) = session.observe_window(|| {});
            inner = Some(coverage);
        });
        assert_eq!(
            inner.as_ref().and_then(FilesystemObserverCoverage::gap),
            Some(&FilesystemObserverGap::ConcurrentObservation)
        );
        assert_eq!(
            outer.gap(),
            Some(&FilesystemObserverGap::ConcurrentObservation)
        );
    }

    #[test]
    fn contention_clears_once_every_window_closes() {
        let (session, _sender) = scripted(Path::new("/repo"));
        let (_, outer) = session.observe_window(|| {
            let _ = session.observe_window(|| {});
        });
        assert!(outer.proven().is_none());
        let (_, later) = session.observe_window(|| {});
        assert!(
            later.proven().is_some(),
            "overlap is a per-window fact, not a permanent loss"
        );
    }

    #[test]
    fn a_file_write_is_file_scoped_and_a_create_is_directory_scoped() {
        let write = notify::Event::new(EventKind::Modify(ModifyKind::Data(DataChange::Content)))
            .add_path(PathBuf::from("/repo/src/lib.rs"));
        assert_eq!(
            classify_notify_event(&write),
            vec![mutated("/repo/src/lib.rs", MutationScope::File)]
        );

        let create = notify::Event::new(EventKind::Create(CreateKind::File))
            .add_path(PathBuf::from("/repo/src/new.rs"));
        assert_eq!(
            classify_notify_event(&create),
            vec![mutated("/repo/src", MutationScope::Directory)],
            "a created entry changes the directory listing a scan may already have read"
        );

        let removed = notify::Event::new(EventKind::Remove(RemoveKind::File))
            .add_path(PathBuf::from("/repo/src/old.rs"));
        assert_eq!(
            classify_notify_event(&removed),
            vec![mutated("/repo/src", MutationScope::Directory)]
        );

        let renamed = notify::Event::new(EventKind::Modify(ModifyKind::Name(RenameMode::Both)))
            .add_path(PathBuf::from("/repo/src/from.rs"))
            .add_path(PathBuf::from("/repo/src/to.rs"));
        assert_eq!(
            classify_notify_event(&renamed),
            vec![
                mutated("/repo/src", MutationScope::Directory),
                mutated("/repo/src", MutationScope::Directory),
            ]
        );
    }

    #[test]
    fn reading_a_file_is_not_a_mutation() {
        let read = notify::Event::new(EventKind::Access(AccessKind::Read))
            .add_path(PathBuf::from("/repo/src/lib.rs"));
        assert_eq!(classify_notify_event(&read), Vec::new());
    }

    #[test]
    fn an_unclassified_event_is_treated_as_a_directory_rescan() {
        let unknown =
            notify::Event::new(EventKind::Any).add_path(PathBuf::from("/repo/src/lib.rs"));
        assert_eq!(
            classify_notify_event(&unknown),
            vec![mutated("/repo/src", MutationScope::Directory)]
        );
    }

    #[test]
    fn a_rescan_flag_outranks_every_path_in_the_same_event() {
        let mut event =
            notify::Event::new(EventKind::Modify(ModifyKind::Data(DataChange::Content)))
                .add_path(PathBuf::from("/repo/src/lib.rs"));
        event = event.set_flag(notify::event::Flag::Rescan);
        assert_eq!(
            classify_notify_event(&event),
            vec![ObservedFilesystemEvent::CoverageLost {
                detail: "platform notifier reported dropped events".to_string(),
            }]
        );
    }

    #[test]
    fn network_and_interop_filesystems_are_classified_apart_from_local_ones() {
        for (name, expected) in [
            ("nfs", Some(UnsupportedFilesystemKind::NetworkMount)),
            ("nfs4", Some(UnsupportedFilesystemKind::NetworkMount)),
            ("cifs", Some(UnsupportedFilesystemKind::NetworkMount)),
            ("smbfs", Some(UnsupportedFilesystemKind::NetworkMount)),
            ("fuse.sshfs", Some(UnsupportedFilesystemKind::NetworkMount)),
            (
                "9p",
                Some(UnsupportedFilesystemKind::WindowsSubsystemForLinuxInterop),
            ),
            (
                "drvfs",
                Some(UnsupportedFilesystemKind::WindowsSubsystemForLinuxInterop),
            ),
            ("fuse", Some(UnsupportedFilesystemKind::UserspaceMount)),
            ("macfuse", Some(UnsupportedFilesystemKind::UserspaceMount)),
            ("apfs", None),
            ("ext4", None),
            ("btrfs", None),
            ("xfs", None),
            ("ntfs", None),
            ("overlay", None),
            ("", None),
        ] {
            assert_eq!(
                classify_filesystem_type_name(name),
                expected,
                "filesystem type {name} was classified wrongly"
            );
        }
    }

    #[test]
    fn the_deepest_mount_that_contains_the_root_decides() {
        let mountinfo = concat!(
            "23 28 0:21 / / rw,relatime shared:1 - ext4 /dev/sda1 rw\n",
            "31 23 0:26 / /work rw,relatime shared:2 - nfs4 fileserver:/work rw\n",
            "32 31 0:27 / /work/local rw,relatime shared:3 - ext4 /dev/sdb1 rw\n",
        );
        assert_eq!(
            classify_mount_table(Path::new("/work/project"), mountinfo),
            Some((UnsupportedFilesystemKind::NetworkMount, "nfs4".to_string()))
        );
        assert_eq!(
            classify_mount_table(Path::new("/work/local/project"), mountinfo),
            None,
            "a local mount nested inside a network mount is still local"
        );
        assert_eq!(classify_mount_table(Path::new("/home/me"), mountinfo), None);
    }

    #[test]
    fn a_mount_point_with_an_escaped_space_still_matches() {
        let mountinfo =
            "40 23 0:44 / /mnt/my\\040share rw,relatime shared:9 - cifs //server/share rw\n";
        assert_eq!(
            classify_mount_table(Path::new("/mnt/my share/repo"), mountinfo),
            Some((UnsupportedFilesystemKind::NetworkMount, "cifs".to_string()))
        );
    }

    #[test]
    fn wsl_drive_interop_is_recognised_from_the_kernel_release() {
        let release = Some("5.15.90.1-microsoft-standard-WSL2");
        assert_eq!(
            classify_wsl_interop(release, Path::new("/mnt/c/Users/me/repo")),
            Some(UnsupportedFilesystemKind::WindowsSubsystemForLinuxInterop)
        );
        assert_eq!(
            classify_wsl_interop(release, Path::new("/home/me/repo")),
            None,
            "the Linux filesystem inside WSL observes normally"
        );
        assert_eq!(
            classify_wsl_interop(release, Path::new("/mnt/storage/repo")),
            None,
            "only single-letter drive mounts are interop mounts"
        );
        assert_eq!(
            classify_wsl_interop(Some("6.1.0-generic"), Path::new("/mnt/c/repo")),
            None,
            "a non-WSL kernel has no drive interop"
        );
        assert_eq!(classify_wsl_interop(None, Path::new("/mnt/c/repo")), None);
    }

    #[test]
    fn unc_and_wsl_windows_roots_are_unsupported_but_local_drives_are_not() {
        assert_eq!(
            classify_windows_path_shape(Path::new(r"\\wsl$\Ubuntu\home\me\repo")),
            Some(UnsupportedFilesystemKind::WindowsSubsystemForLinuxInterop)
        );
        assert_eq!(
            classify_windows_path_shape(Path::new(r"\\wsl.localhost\Ubuntu\home\me\repo")),
            Some(UnsupportedFilesystemKind::WindowsSubsystemForLinuxInterop)
        );
        assert_eq!(
            classify_windows_path_shape(Path::new(r"\\fileserver\share\repo")),
            Some(UnsupportedFilesystemKind::NetworkMount)
        );
        assert_eq!(
            classify_windows_path_shape(Path::new(r"\\?\UNC\fileserver\share\repo")),
            Some(UnsupportedFilesystemKind::NetworkMount)
        );
        assert_eq!(
            classify_windows_path_shape(Path::new(r"\\?\C:\repo")),
            None,
            "the extended-length local prefix is not a network root"
        );
        assert_eq!(classify_windows_path_shape(Path::new(r"C:\repo")), None);
    }

    #[test]
    fn arming_refuses_an_unsupported_filesystem_before_it_creates_a_notifier() {
        // The root is a perfectly observable local directory. Only the filesystem description
        // says otherwise, which is what an NFS or WSL drive mount looks like to `arm`.
        let root = tempfile::tempdir().expect("temporary root");
        for (observed, expected) in [
            (
                ObservedFilesystem {
                    type_name: Some("nfs4".to_string()),
                    ..ObservedFilesystem::default()
                },
                UnsupportedFilesystemKind::NetworkMount,
            ),
            (
                ObservedFilesystem {
                    mount_table: Some(format!(
                        "23 28 0:21 / / rw shared:1 - ext4 /dev/sda1 rw\n\
                         31 23 0:26 / {} rw shared:2 - cifs //server/share rw\n",
                        root.path().display()
                    )),
                    ..ObservedFilesystem::default()
                },
                UnsupportedFilesystemKind::NetworkMount,
            ),
            (
                ObservedFilesystem {
                    os_release: Some("5.15.90.1-microsoft-standard-WSL2".to_string()),
                    ..ObservedFilesystem::default()
                },
                UnsupportedFilesystemKind::WindowsSubsystemForLinuxInterop,
            ),
        ] {
            let root = if expected == UnsupportedFilesystemKind::WindowsSubsystemForLinuxInterop {
                PathBuf::from("/mnt/c/Users/me/repo")
            } else {
                root.path().to_path_buf()
            };
            let gap = FilesystemObserverSession::arm_with_filesystem(&root, &observed)
                .expect_err("an unsupported filesystem must refuse to arm");
            assert_eq!(
                gap,
                FilesystemObserverGap::UnsupportedFilesystem {
                    kind: expected,
                    detail: match (&observed.type_name, &observed.mount_table) {
                        (Some(name), _) => name.clone(),
                        (None, Some(_)) => "cifs".to_string(),
                        (None, None) => "windows drive interop mount".to_string(),
                    },
                }
            );
        }
    }

    #[test]
    fn arming_accepts_a_local_filesystem() {
        let root = tempfile::tempdir().expect("temporary root");
        let session = FilesystemObserverSession::arm_with_filesystem(
            root.path(),
            &ObservedFilesystem {
                type_name: Some("apfs".to_string()),
                os_release: Some("6.1.0-generic".to_string()),
                mount_table: Some("23 28 0:21 / / rw shared:1 - ext4 /dev/sda1 rw\n".to_string()),
            },
        )
        .expect("a local filesystem must arm");
        assert_eq!(
            session.identity().backend(),
            FilesystemObserverBackend::Native
        );
    }

    #[test]
    fn every_gap_carries_a_distinct_stable_identity() {
        let gaps = [
            FilesystemObserverGap::BackendUnavailable {
                detail: String::new(),
            },
            FilesystemObserverGap::UnsupportedFilesystem {
                kind: UnsupportedFilesystemKind::NetworkMount,
                detail: String::new(),
            },
            FilesystemObserverGap::EventQueueOverflow {
                detail: String::new(),
            },
            FilesystemObserverGap::BackendFailed {
                detail: String::new(),
            },
            FilesystemObserverGap::ObservationBudgetExhausted { limit: 1 },
            FilesystemObserverGap::ConcurrentObservation,
        ];
        let identities = gaps
            .iter()
            .map(FilesystemObserverGap::id)
            .collect::<BTreeSet<_>>();
        assert_eq!(identities.len(), gaps.len());
    }

    #[test]
    fn the_native_observer_reports_a_write_that_races_a_scan() {
        // The scripted source hands the session paths the test chose. Only a real notifier over a
        // real directory proves the session lines observed paths up with the caller's spelling,
        // which on macOS reaches the temporary directory through a symlink.
        let root = tempfile::tempdir().expect("temporary root");
        let source = root.path().join("src");
        std::fs::create_dir(&source).expect("source directory");
        std::fs::write(source.join("lib.rs"), "fn existing() {}\n").expect("seed file");

        let session = match FilesystemObserverSession::arm(root.path()) {
            Ok(session) => session,
            Err(gap) => panic!(
                "the host must observe a temporary directory: {}",
                gap.detail()
            ),
        };
        assert_eq!(
            session.identity().backend(),
            FilesystemObserverBackend::Native
        );

        let target = source.join("lib.rs");
        let (_, coverage) = session.observe_window(|| {
            // Stand in for the authoritative scan: write while the window is open, then wait for
            // the notifier to deliver rather than assuming a delivery latency.
            std::fs::write(&target, "fn racing() {}\n").expect("racing write");
            let deadline = Instant::now() + Duration::from_secs(20);
            while Instant::now() < deadline && session.epoch() == 0 {
                std::thread::sleep(Duration::from_millis(25));
            }
        });

        let proven = coverage
            .proven()
            .expect("a local temporary directory must observe without loss");
        assert!(
            !proven.is_quiet(),
            "the write inside the window must advance the epoch"
        );
        let observed = proven
            .dirty_paths()
            .iter()
            .chain(proven.rescan_roots().iter())
            .cloned()
            .collect::<BTreeSet<_>>();
        assert!(
            observed.contains(&target) || observed.contains(&source),
            "observed paths {observed:?} must be spelled under the armed root {}",
            root.path().display()
        );
    }
}
