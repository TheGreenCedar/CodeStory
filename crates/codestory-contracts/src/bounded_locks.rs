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

use std::fmt;
use std::fs::File;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use fs4::fs_std::FileExt;

/// Default wait budget for a lock that guards a routine sibling operation.
/// Long enough to absorb a peer's publication commit, short enough that a
/// wedged holder cannot outlive one foreground request.
pub const DEFAULT_LOCK_WAIT: Duration = Duration::from_secs(10);

/// Wait budget for a lock a peer holds for the length of a whole publication
/// or cleanup pass. Only background workers wait this long; a foreground
/// request reaches its own preparing verdict long before.
pub const PUBLICATION_LOCK_WAIT: Duration = Duration::from_secs(120);

/// First re-poll gap. A concurrently spawned child inherits open descriptors
/// until `exec` applies `O_CLOEXEC`, so a ghost conflict clears within
/// microseconds; starting small keeps that case free.
const FIRST_POLL_STEP: Duration = Duration::from_millis(2);

/// Longest gap between re-polls. Bounds wake-ups on a long wait without
/// letting the observed acquisition lag the real release by much.
const MAX_POLL_STEP: Duration = Duration::from_millis(25);

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

/// Acquire `kind` on `file`, waiting no longer than `deadline` and abandoning
/// the wait as soon as `cancel` is raised.
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
