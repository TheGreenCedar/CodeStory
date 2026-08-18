//! Non-blocking file-lock acquisition that outlives fork/exec ghost holds.

use codestory_contracts::bounded_locks::{
    FileLockError, FileLockKind, LockDeadline, acquire_with_deadline,
};
use std::fs::File;
use std::io;
use std::time::Duration;

/// One try-lock attempt can spuriously report contention: a concurrently
/// spawned child process (fork then exec) inherits every open descriptor,
/// including a sibling thread's `flock`ed lock, until exec applies
/// `O_CLOEXEC`. Darwin's `posix_spawn` applies close-on-exec atomically in
/// the kernel; Linux does not, so any process that spawns children while
/// another thread acquires a lock sees ghost conflicts that clear within
/// microseconds. A genuine holder keeps the lock far past this bounded
/// budget, so retrying preserves fail-closed semantics: the final attempt's
/// verdict is authoritative.
const SPAWN_GHOST_BUDGET: Duration = Duration::from_millis(40);

pub fn try_lock_exclusive_outliving_spawn_ghosts(file: &File) -> io::Result<bool> {
    match acquire_with_deadline(
        file,
        FileLockKind::Exclusive,
        LockDeadline::after(SPAWN_GHOST_BUDGET),
        None,
    ) {
        Ok(()) => Ok(true),
        Err(FileLockError::Timeout { .. }) => Ok(false),
        // No cancellation flag is supplied here, so only a platform refusal
        // can reach this arm; it stays an error rather than a busy verdict.
        Err(error) => Err(io::Error::other(error.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codestory_contracts::bounded_locks::{release, try_acquire};
    use std::sync::mpsc;
    use std::thread;

    fn lock_file() -> (tempfile::TempDir, File) {
        let dir = tempfile::tempdir().expect("lock dir");
        let path = dir.path().join("subject.lock");
        let file = File::create(&path).expect("create lock");
        (dir, file)
    }

    #[test]
    fn briefly_held_lock_is_acquired_within_the_budget() {
        let (dir, file) = lock_file();
        let holder = File::open(dir.path().join("subject.lock")).expect("holder handle");
        assert!(try_acquire(&holder, FileLockKind::Exclusive).expect("holder locks"));
        let (release_started, release_gate) = mpsc::channel();
        let releaser = thread::spawn(move || {
            release_started.send(()).expect("signal release start");
            thread::sleep(Duration::from_millis(6));
            drop(holder);
        });
        release_gate.recv().expect("holder release scheduled");

        assert!(
            try_lock_exclusive_outliving_spawn_ghosts(&file).expect("retrying acquire"),
            "a ghost-lived hold must clear within the retry budget"
        );
        releaser.join().expect("releaser thread");
        release(&file).expect("release waiter");
    }

    #[test]
    fn genuinely_held_lock_still_fails_closed() {
        let (dir, file) = lock_file();
        let holder = File::open(dir.path().join("subject.lock")).expect("holder handle");
        assert!(try_acquire(&holder, FileLockKind::Exclusive).expect("holder locks"));

        assert!(
            !try_lock_exclusive_outliving_spawn_ghosts(&file).expect("bounded acquire"),
            "a persistent holder must still report contention"
        );
        drop(holder);
    }
}
