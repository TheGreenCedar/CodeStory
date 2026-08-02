//! The single bounded entry point for advisory file locks.
//!
//! Every advisory lock CodeStory takes — publication, promotion, retention,
//! search generations, diagnostics, model materialization — is acquired here.
//! A blocking `flock` has no timeout and no cancellation poll, so one stalled
//! sibling process could hold an unrelated request, an eviction, or shutdown
//! for as long as it liked. Acquisition therefore always carries an absolute
//! deadline and an optional cancellation flag, and reports refusal with a
//! typed code instead of an opaque `io::Error`.
//!
//! This module owns the only `fs4` import in production source; an
//! architecture contract enforces that.

use std::cell::RefCell;
use std::fmt;
use std::fs::File;
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use fs4::fs_std::FileExt;

/// Default wait budget for a lock whose critical section is a single small
/// read-modify-write — a state file, a status guard, a diagnostics slot. A
/// holder that needs longer than this is wedged, not busy.
pub const DEFAULT_LOCK_WAIT: Duration = Duration::from_secs(10);

/// Wait budget for a lock a peer holds for the length of a whole publication,
/// promotion, or retention pass. Every waiter that can be behind such a pass
/// uses this: a legitimate commit routinely outlasts [`DEFAULT_LOCK_WAIT`], so
/// a shorter budget would turn ordinary contention into a hard failure.
///
/// A wait this long is only safe because it is interruptible: the waiter either
/// passes its own flag or inherits one from [`with_thread_cancellation`], so a
/// cancelled request, eviction, or shutdown ends it within
/// [`MAX_CANCELLATION_LATENCY`] instead of the peer's hold.
pub const PUBLICATION_LOCK_WAIT: Duration = Duration::from_secs(120);

/// First re-poll gap. A concurrently spawned child inherits open descriptors
/// until `exec` applies `O_CLOEXEC`, so a ghost conflict clears within
/// microseconds; starting small keeps that case free.
const FIRST_POLL_STEP: Duration = Duration::from_millis(2);

/// Longest gap between re-polls. Bounds wake-ups on a long wait without
/// letting the observed acquisition lag the real release by much.
const MAX_POLL_STEP: Duration = Duration::from_millis(25);

/// Longest a bounded acquisition keeps waiting after its cancellation source is
/// raised. The wait sleeps only in re-poll gaps and re-reads the flag at the
/// top of every one, so a raised flag is observed within one gap.
///
/// This is the whole reason a [`PUBLICATION_LOCK_WAIT`] is not a liveness
/// hazard: a joiner that cancels a worker and then waits for it to quiesce must
/// only budget past this, not past the lock's wait budget. Any such joiner is
/// expected to assert its budget exceeds this constant.
pub const MAX_CANCELLATION_LATENCY: Duration = MAX_POLL_STEP;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileLockKind {
    Shared,
    Exclusive,
}

impl FileLockKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Shared => "shared",
            Self::Exclusive => "exclusive",
        }
    }
}

impl fmt::Display for FileLockKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Why a bounded acquisition did not produce a held lock.
#[derive(Debug)]
pub enum FileLockError {
    /// The wait budget expired while another holder kept the lock.
    Timeout {
        kind: FileLockKind,
        waited: Duration,
    },
    /// The caller's cancellation flag was raised while waiting.
    Cancelled {
        kind: FileLockKind,
        waited: Duration,
    },
    /// The platform refused the lock operation outright.
    Unavailable {
        kind: FileLockKind,
        source: io::Error,
    },
}

impl FileLockError {
    /// Stable, machine-matchable failure code.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Timeout { .. } => "lock_wait_timeout",
            Self::Cancelled { .. } => "lock_wait_cancelled",
            Self::Unavailable { .. } => "lock_unavailable",
        }
    }

    pub fn is_timeout(&self) -> bool {
        matches!(self, Self::Timeout { .. })
    }

    pub fn is_cancelled(&self) -> bool {
        matches!(self, Self::Cancelled { .. })
    }
}

impl fmt::Display for FileLockError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Timeout { kind, waited } => write!(
                formatter,
                "lock_wait_timeout: another holder kept the {kind} lock for the whole {} ms budget",
                waited.as_millis()
            ),
            Self::Cancelled { kind, waited } => write!(
                formatter,
                "lock_wait_cancelled: the {kind} lock wait was cancelled after {} ms",
                waited.as_millis()
            ),
            Self::Unavailable { kind, source } => {
                write!(
                    formatter,
                    "lock_unavailable: the {kind} lock could not be taken: {source}"
                )
            }
        }
    }
}

impl std::error::Error for FileLockError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Unavailable { source, .. } => Some(source),
            Self::Timeout { .. } | Self::Cancelled { .. } => None,
        }
    }
}

/// Absolute point in time by which acquisition must succeed or report refusal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LockDeadline {
    at: Instant,
}

impl LockDeadline {
    pub fn after(budget: Duration) -> Self {
        Self {
            // A saturating add keeps an absurd budget from wrapping into an
            // already-expired deadline.
            at: Instant::now()
                .checked_add(budget)
                .unwrap_or_else(Instant::now),
        }
    }

    /// Expires immediately: one non-blocking attempt, no waiting.
    pub fn immediate() -> Self {
        Self { at: Instant::now() }
    }

    fn remaining(&self) -> Duration {
        self.at.saturating_duration_since(Instant::now())
    }
}

thread_local! {
    static THREAD_CANCELLATION: RefCell<Option<Arc<AtomicBool>>> = const { RefCell::new(None) };
}

struct ThreadCancellationGuard {
    previous: Option<Arc<AtomicBool>>,
}

impl Drop for ThreadCancellationGuard {
    fn drop(&mut self) {
        THREAD_CANCELLATION.with(|active| {
            active.replace(self.previous.take());
        });
    }
}

/// Run `body` with `cancel` as the ambient cancellation for every bounded lock
/// acquisition this thread performs, however deep.
///
/// A lock wait several crates below the caller has no cancellation flag in its
/// signature and cannot grow one without a cross-crate cascade, yet it is
/// exactly where a cancelled worker gets stuck: `PromotionLock`, the search
/// index and generation-catalog guards, retention, and model materialization
/// are all reached through APIs that never took a flag. Installing the flag on
/// the thread reaches all of them, and reaches call sites added later that
/// nobody remembered to plumb.
///
/// An explicit `cancel` argument still wins where a call site has one; this is
/// the floor, not a replacement.
pub fn with_thread_cancellation<T>(cancel: Arc<AtomicBool>, body: impl FnOnce() -> T) -> T {
    let previous = THREAD_CANCELLATION.with(|active| active.replace(Some(cancel)));
    let _guard = ThreadCancellationGuard { previous };
    body()
}

/// The ambient cancellation installed by the innermost
/// [`with_thread_cancellation`] on this thread, if any.
pub fn thread_cancellation() -> Option<Arc<AtomicBool>> {
    THREAD_CANCELLATION.with(|active| active.borrow().clone())
}

/// Acquire `kind` on `file`, waiting no longer than `deadline` and abandoning
/// the wait as soon as a cancellation source is raised.
///
/// The cancellation source is `cancel` when the call site has one, and this
/// thread's [`with_thread_cancellation`] flag otherwise. A wait with neither is
/// uninterruptible for its whole budget, so no thread whose quiescence is
/// joined against a deadline may reach one — see the assertion beside
/// `ACTIVATION_QUIESCENCE_BUDGET`.
///
/// This is the only blocking-capable lock acquisition in production source.
/// It never calls `fs4`'s blocking `lock_shared`/`lock_exclusive`: those cannot
/// observe a deadline or a cancellation flag once they enter the kernel.
pub fn acquire_with_deadline(
    file: &File,
    kind: FileLockKind,
    deadline: LockDeadline,
    cancel: Option<&AtomicBool>,
) -> Result<(), FileLockError> {
    let inherited = cancel.is_none().then(thread_cancellation).flatten();
    let cancel = cancel.or(inherited.as_deref());
    let started = Instant::now();
    let mut step = FIRST_POLL_STEP;
    loop {
        if cancelled(cancel) {
            return Err(FileLockError::Cancelled {
                kind,
                waited: started.elapsed(),
            });
        }
        if try_acquire(file, kind)? {
            return Ok(());
        }
        let remaining = deadline.remaining();
        if remaining.is_zero() {
            return Err(FileLockError::Timeout {
                kind,
                waited: started.elapsed(),
            });
        }
        std::thread::sleep(step.min(remaining));
        step = (step * 2).min(MAX_POLL_STEP);
    }
}

/// One non-blocking attempt. Reports contention as `Ok(false)` so callers can
/// keep their fail-closed "busy" verdicts.
pub fn try_acquire(file: &File, kind: FileLockKind) -> Result<bool, FileLockError> {
    let attempt = match kind {
        FileLockKind::Shared => FileExt::try_lock_shared(file),
        FileLockKind::Exclusive => FileExt::try_lock_exclusive(file),
    };
    match attempt {
        Ok(acquired) => Ok(acquired),
        // Some platforms report contention as an error rather than `false`.
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(false),
        Err(source) => Err(FileLockError::Unavailable { kind, source }),
    }
}

/// Release a lock held on `file`.
pub fn release(file: &File) -> Result<(), FileLockError> {
    FileExt::unlock(file).map_err(|source| FileLockError::Unavailable {
        kind: FileLockKind::Exclusive,
        source,
    })
}

/// Convert an exclusive hold into a shared one without a release window on
/// platforms whose `flock` atomically downgrades.
pub fn downgrade_to_shared(file: &File, deadline: LockDeadline) -> Result<(), FileLockError> {
    #[cfg(unix)]
    {
        acquire_with_deadline(file, FileLockKind::Shared, deadline, None)
    }
    #[cfg(not(unix))]
    {
        release(file)?;
        acquire_with_deadline(file, FileLockKind::Shared, deadline, None)
    }
}

fn cancelled(cancel: Option<&AtomicBool>) -> bool {
    cancel.is_some_and(|flag| flag.load(Ordering::Acquire))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::OpenOptions;
    use std::sync::Arc;
    use std::sync::mpsc;
    use std::thread;

    fn lock_file(dir: &std::path::Path) -> File {
        OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(dir.join("subject.lock"))
            .expect("open lock file")
    }

    #[test]
    fn a_free_lock_is_acquired_immediately() {
        let dir = tempfile::tempdir().expect("lock dir");
        let file = lock_file(dir.path());
        acquire_with_deadline(
            &file,
            FileLockKind::Exclusive,
            LockDeadline::immediate(),
            None,
        )
        .expect("free lock");
        release(&file).expect("release");
    }

    #[test]
    fn a_held_lock_times_out_with_a_typed_code_instead_of_blocking() {
        let dir = tempfile::tempdir().expect("lock dir");
        let holder = lock_file(dir.path());
        assert!(
            try_acquire(&holder, FileLockKind::Exclusive).expect("holder locks"),
            "the holder must take the lock first"
        );
        let waiter = lock_file(dir.path());

        let started = Instant::now();
        let error = acquire_with_deadline(
            &waiter,
            FileLockKind::Exclusive,
            LockDeadline::after(Duration::from_millis(120)),
            None,
        )
        .expect_err("a persistently held lock must refuse");

        assert_eq!(error.code(), "lock_wait_timeout");
        assert!(error.is_timeout());
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "acquisition must return on its own deadline, not the holder's lifetime"
        );
        release(&holder).expect("release holder");
    }

    #[test]
    fn a_raised_cancel_flag_abandons_the_wait_before_the_deadline() {
        let dir = tempfile::tempdir().expect("lock dir");
        let holder = lock_file(dir.path());
        assert!(try_acquire(&holder, FileLockKind::Exclusive).expect("holder locks"));
        let waiter = lock_file(dir.path());
        let cancel = Arc::new(AtomicBool::new(false));
        let raiser = Arc::clone(&cancel);
        let (raised, raised_rx) = mpsc::channel();
        let canceller = thread::spawn(move || {
            thread::sleep(Duration::from_millis(10));
            raiser.store(true, Ordering::Release);
            raised.send(()).expect("signal cancellation");
        });

        let started = Instant::now();
        let error = acquire_with_deadline(
            &waiter,
            FileLockKind::Exclusive,
            // A budget far past the cancellation so only the flag can end the wait.
            LockDeadline::after(Duration::from_secs(30)),
            Some(cancel.as_ref()),
        )
        .expect_err("a cancelled wait must refuse");

        raised_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("cancellation raised");
        canceller.join().expect("cancellation thread");
        assert_eq!(error.code(), "lock_wait_cancelled");
        assert!(error.is_cancelled());
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "cancellation must end the wait long before the deadline"
        );
        release(&holder).expect("release holder");
    }

    #[test]
    fn a_briefly_held_lock_is_acquired_within_the_budget() {
        let dir = tempfile::tempdir().expect("lock dir");
        let holder = lock_file(dir.path());
        assert!(try_acquire(&holder, FileLockKind::Exclusive).expect("holder locks"));
        let waiter = lock_file(dir.path());
        let releaser = thread::spawn(move || {
            thread::sleep(Duration::from_millis(20));
            release(&holder).expect("release holder");
            drop(holder);
        });

        acquire_with_deadline(
            &waiter,
            FileLockKind::Exclusive,
            LockDeadline::after(Duration::from_secs(30)),
            None,
        )
        .expect("a released lock must be acquired");
        releaser.join().expect("releaser thread");
        release(&waiter).expect("release waiter");
    }

    /// The realistic shape of the bug this exists to prevent: a call site
    /// several crates below the cancelling worker passes `None` because its
    /// signature has no flag. Without ambient inheritance it waits out the
    /// whole publication budget and the worker is declared unquiesced.
    #[test]
    fn a_call_site_with_no_flag_inherits_the_threads_cancellation() {
        let dir = tempfile::tempdir().expect("lock dir");
        let holder = lock_file(dir.path());
        assert!(try_acquire(&holder, FileLockKind::Exclusive).expect("holder locks"));
        let waiter = lock_file(dir.path());
        let cancel = Arc::new(AtomicBool::new(false));
        let raiser = Arc::clone(&cancel);
        let canceller = thread::spawn(move || {
            thread::sleep(Duration::from_millis(10));
            raiser.store(true, Ordering::Release);
        });

        let started = Instant::now();
        let error = with_thread_cancellation(Arc::clone(&cancel), || {
            // Exactly what a deep call site does today: no flag of its own and
            // the full publication budget.
            acquire_with_deadline(
                &waiter,
                FileLockKind::Exclusive,
                LockDeadline::after(PUBLICATION_LOCK_WAIT),
                None,
            )
            .expect_err("an inherited cancellation must end the wait")
        });
        let waited = started.elapsed();

        canceller.join().expect("cancellation thread");
        assert_eq!(error.code(), "lock_wait_cancelled");
        assert!(
            waited < Duration::from_secs(5),
            "the wait ran {waited:?} instead of ending on the inherited flag"
        );
        release(&holder).expect("release holder");
    }

    #[test]
    fn the_thread_cancellation_scope_is_restored_when_the_body_returns() {
        let outer = Arc::new(AtomicBool::new(false));
        let inner = Arc::new(AtomicBool::new(false));
        assert!(thread_cancellation().is_none());
        with_thread_cancellation(Arc::clone(&outer), || {
            assert!(Arc::ptr_eq(
                &thread_cancellation().expect("outer scope"),
                &outer
            ));
            with_thread_cancellation(Arc::clone(&inner), || {
                assert!(Arc::ptr_eq(
                    &thread_cancellation().expect("inner scope"),
                    &inner
                ));
            });
            assert!(Arc::ptr_eq(
                &thread_cancellation().expect("outer scope restored"),
                &outer
            ));
        });
        assert!(thread_cancellation().is_none());
    }

    /// A joiner that cancels a worker and then waits for it to quiesce budgets
    /// against this, not against the lock's wait budget. If the re-poll gap
    /// ever grew past it the published guarantee would be false.
    #[test]
    fn a_raised_flag_is_observed_within_the_published_cancellation_latency() {
        assert_eq!(MAX_CANCELLATION_LATENCY, MAX_POLL_STEP);
        let dir = tempfile::tempdir().expect("lock dir");
        let holder = lock_file(dir.path());
        assert!(try_acquire(&holder, FileLockKind::Exclusive).expect("holder locks"));
        let waiter = lock_file(dir.path());
        // Raised before the wait even starts: the very first poll must see it.
        let cancel = AtomicBool::new(true);

        let started = Instant::now();
        let error = acquire_with_deadline(
            &waiter,
            FileLockKind::Exclusive,
            LockDeadline::after(PUBLICATION_LOCK_WAIT),
            Some(&cancel),
        )
        .expect_err("an already-raised flag must refuse immediately");
        assert_eq!(error.code(), "lock_wait_cancelled");
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "an already-raised flag must not enter a re-poll gap at all"
        );
        release(&holder).expect("release holder");
    }

    #[test]
    fn shared_holders_do_not_exclude_each_other() {
        let dir = tempfile::tempdir().expect("lock dir");
        let first = lock_file(dir.path());
        let second = lock_file(dir.path());
        acquire_with_deadline(
            &first,
            FileLockKind::Shared,
            LockDeadline::immediate(),
            None,
        )
        .expect("first shared");
        acquire_with_deadline(
            &second,
            FileLockKind::Shared,
            LockDeadline::immediate(),
            None,
        )
        .expect("second shared");
        release(&first).expect("release first");
        release(&second).expect("release second");
    }
}
