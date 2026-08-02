//! Non-blocking file-lock acquisition that outlives fork/exec ghost holds.

use fs4::fs_std::FileExt as _;
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
pub fn try_lock_exclusive_outliving_spawn_ghosts(file: &File) -> io::Result<bool> {
    try_lock_with_budget(file, 20, Duration::from_millis(2))
}

fn try_lock_with_budget(file: &File, retries: u32, step: Duration) -> io::Result<bool> {
    for _ in 0..retries {
        if file.try_lock_exclusive()? {
            return Ok(true);
        }
        std::thread::sleep(step);
    }
    file.try_lock_exclusive()
}

#[cfg(test)]
mod tests {
    use super::*;
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
        assert!(holder.try_lock_exclusive().expect("holder locks"));
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
    }

    #[test]
    fn genuinely_held_lock_still_fails_closed() {
        let (dir, file) = lock_file();
        let holder = File::open(dir.path().join("subject.lock")).expect("holder handle");
        assert!(holder.try_lock_exclusive().expect("holder locks"));

        assert!(
            !try_lock_with_budget(&file, 3, Duration::from_millis(1)).expect("bounded acquire"),
            "a persistent holder must still report contention"
        );
        drop(holder);
    }
}
