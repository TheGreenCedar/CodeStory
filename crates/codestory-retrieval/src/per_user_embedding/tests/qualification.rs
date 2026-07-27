use super::super::{
    CommandAbsence, MAX_DENIED_COMMAND_TICKS, absent_command, read_server_qualification_command,
    server_qualification_control_from_values,
};
// Only the platform-gated tests below use these, so importing them
// unconditionally would warn on whichever platform is not running them.
#[cfg(windows)]
use super::super::native_path_identity;
#[cfg(unix)]
use super::super::{
    SERVER_QUALIFICATION_MAX_COMMAND_BYTES, SERVER_QUALIFICATION_MAX_EVENT_BYTES,
    SERVER_QUALIFICATION_MAX_EVENT_RECORDS, hex_sha256,
};
use super::{test_qualification_control, test_qualification_event};
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;

/// Write one command file already private, before anything can observe it.
fn write_private_command(path: &Path, bytes: &[u8]) {
    fs::write(path, bytes).expect("write qualification command");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .expect("private qualification command");
    }
}

/// A command the proof harness takes back mid-consume must read as "no command
/// this tick", never as a fatal poll error.
///
/// The harness removes each command once it has seen the matching event, and
/// that removal races every poll tick that has already inspected the file.
/// Before the vanish tolerance the consume sequence turned the loser of that
/// race into an error, which propagates out of the accept loop and kills the
/// server -- leaving the harness waiting out its whole budget on a producer
/// that no longer exists.
#[test]
fn a_command_removed_mid_consume_never_kills_the_poll_loop() {
    const ROUNDS: usize = 4_096;

    let (_temporary, control) = test_qualification_control();
    let command_path = control
        .directory
        .join(format!("{}.command.json", control.nonce));
    let round = AtomicUsize::new(0);
    let removed = AtomicUsize::new(0);
    let stop = AtomicBool::new(false);
    let consumed = AtomicUsize::new(0);
    let absent = AtomicUsize::new(0);

    thread::scope(|scope| {
        let remover_path = command_path.clone();
        let remover = &round;
        let acknowledged = &removed;
        let halt = &stop;
        scope.spawn(move || {
            let mut seen = 0;
            loop {
                let mut waited = 0_u32;
                while remover.load(Ordering::Acquire) == seen {
                    if halt.load(Ordering::Acquire) {
                        return;
                    }
                    // Spin to keep the handshake tight enough to land inside a
                    // consume, but yield periodically so a single-core host
                    // still makes progress.
                    waited = waited.wrapping_add(1);
                    if waited.is_multiple_of(1_024) {
                        thread::yield_now();
                    } else {
                        std::hint::spin_loop();
                    }
                }
                seen = remover.load(Ordering::Acquire);
                // Sweep the removal across the whole consume sequence and past
                // its end, so across the run it lands before the first stat,
                // between the stat and the open, while the handle is open, and
                // after the consume has already finished.
                for _ in 0..((seen % 512) * 64) {
                    std::hint::spin_loop();
                }
                let _ = fs::remove_file(&remover_path);
                acknowledged.store(seen, Ordering::Release);
            }
        });

        for index in 1..=ROUNDS {
            write_private_command(&command_path, b"{\"schema_version\":1}");
            round.store(index, Ordering::Release);
            match read_server_qualification_command(&control) {
                Ok(Some(_)) => {
                    consumed.fetch_add(1, Ordering::Release);
                }
                Ok(None) => {
                    absent.fetch_add(1, Ordering::Release);
                }
                Err(error) => {
                    stop.store(true, Ordering::Release);
                    panic!("round {index} killed the poll loop: {error:#}");
                }
            }
            let mut waited = 0_u32;
            while removed.load(Ordering::Acquire) != index {
                waited = waited.wrapping_add(1);
                if waited.is_multiple_of(1_024) {
                    thread::yield_now();
                } else {
                    std::hint::spin_loop();
                }
            }
            let _ = fs::remove_file(&command_path);
        }
        stop.store(true, Ordering::Release);
        round.fetch_add(1, Ordering::Release);
    });

    assert!(
        consumed.load(Ordering::Acquire) > 0,
        "the race never let one command through, so nothing was consumed"
    );
    assert!(
        absent.load(Ordering::Acquire) > 0,
        "the race never removed a command in time, so the tolerance was untested"
    );
}

/// The vanish tolerance must not become "ignore every filesystem error".
///
/// A command that is present but unreadable is a real control-plane failure:
/// tolerating it would convert the poll loop's genuine faults into silent
/// waits, which is worse than the crash the tolerance removes.
#[cfg(unix)]
#[test]
fn a_present_but_unreadable_command_still_stops_the_server() {
    use std::os::unix::fs::PermissionsExt;

    if unsafe { libc::geteuid() } == 0 {
        // Root ignores the mode, so this host cannot produce the denial.
        return;
    }
    let (_temporary, control) = test_qualification_control();
    let command_path = control
        .directory
        .join(format!("{}.command.json", control.nonce));
    write_private_command(&command_path, b"{\"schema_version\":1}");
    // Still owner-only and still exactly one link, so only the read is denied.
    fs::set_permissions(&command_path, fs::Permissions::from_mode(0o000))
        .expect("deny reads on a present command");
    let error = read_server_qualification_command(&control)
        .expect_err("an unreadable present command is not a vanished command");
    assert!(
        format!("{error:#}").contains("open embedding qualification command"),
        "unexpected error: {error:#}"
    );
}

/// A Windows `ACCESS_DENIED` is only evidence of a removal in progress for as
/// long as a delete-pending entry could plausibly last.
///
/// The tolerance cannot distinguish a delete-pending entry from a file whose
/// ACL simply refuses the open, so an unbounded tolerance would skip a genuinely
/// unreadable command on every tick forever -- reproducing, inside the server,
/// exactly the unattributable multi-minute hang this lane exists to remove. The
/// bound is driven directly here because no Unix host can produce the Windows
/// observation that feeds it.
#[test]
fn a_denial_that_outlives_a_delete_pending_entry_stops_the_server() {
    let (_temporary, control) = test_qualification_control();
    for tick in 1..=MAX_DENIED_COMMAND_TICKS {
        assert!(
            absent_command(&control, CommandAbsence::Denied)
                .unwrap_or_else(|error| panic!("denied tick {tick} was not tolerated: {error:#}"))
                .is_none(),
            "a denied tick produced a command"
        );
    }
    let error = absent_command(&control, CommandAbsence::Denied)
        .expect_err("a denial past the bound is not a removal in progress");
    assert!(
        format!("{error:#}").contains("embedding_qualification_command_denied"),
        "unexpected error: {error:#}"
    );

    // A proven removal is the ordinary case and must clear the run, so routine
    // command cleanup can never accumulate its way into that failure however
    // many times it races a poll tick.
    for round in 1..=3 {
        absent_command(&control, CommandAbsence::Removed).expect("a removal is never a failure");
        for tick in 1..=MAX_DENIED_COMMAND_TICKS {
            absent_command(&control, CommandAbsence::Denied).unwrap_or_else(|error| {
                panic!("round {round} tick {tick} outlived a reset run: {error:#}")
            });
        }
    }
}

/// A different file substituted at the pinned path is a substitution, not a
/// vanish, and must still stop the server.
#[test]
fn a_non_file_at_the_pinned_command_path_still_stops_the_server() {
    let (_temporary, control) = test_qualification_control();
    let command_path = control
        .directory
        .join(format!("{}.command.json", control.nonce));
    fs::create_dir(&command_path).expect("directory at the pinned command path");
    let error = read_server_qualification_command(&control)
        .expect_err("a directory is not a vanished command");
    assert!(
        format!("{error:#}").contains("embedding_qualification_file_untrusted"),
        "unexpected error: {error:#}"
    );
}

/// The Windows removal the proof harness actually performs must read as "no
/// command this tick" whichever way the filesystem answers it.
///
/// `os.unlink` calls `DeleteFileW`, and what that leaves behind is not the same
/// on every host. Measured on the Windows proof host (Windows 11 build
/// 26200.8894, NTFS, `C:`): `DeleteFileW` takes POSIX delete semantics, so the
/// name is unlinked at once and every later observation -- `symlink_metadata`,
/// the attributes-only open native path identity performs, and a read open --
/// answers `NotFound`. On a volume or Windows version without POSIX delete the
/// classic behaviour applies instead: the entry stays visible and delete-pending
/// while a handle is open, and answers those same opens with `ACCESS_DENIED`.
///
/// Both are the writer taking its command back, so this asserts the outcome and
/// records the regime rather than assuming one of them. Assuming the classic
/// regime is what an earlier version of this test did, and on the measured host
/// that assumption is simply false.
#[cfg(windows)]
#[test]
fn a_removed_command_never_kills_the_poll_loop_on_windows() {
    use std::os::windows::ffi::OsStrExt;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn DeleteFileW(file_name: *const u16) -> i32;
    }

    let (_temporary, control) = test_qualification_control();
    let command_path = control
        .directory
        .join(format!("{}.command.json", control.nonce));
    write_private_command(&command_path, b"{\"schema_version\":1}");
    // Hold the command open across the removal, which is the state a poll tick
    // that has already opened the file is in when the harness takes it back.
    let held = fs::File::open(&command_path).expect("hold the command open");
    let wide: Vec<u16> = command_path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // Exactly the call `os.unlink` makes, not std's remove_file, so the test
    // observes whatever the harness itself produces on this host.
    assert_ne!(
        unsafe { DeleteFileW(wide.as_ptr()) },
        0,
        "DeleteFileW failed: {}",
        std::io::Error::last_os_error()
    );
    let regime = match fs::symlink_metadata(&command_path) {
        Ok(_) => "delete-pending: the entry is still visible",
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            "posix delete: the name was unlinked at once"
        }
        Err(error) => panic!("unexpected observation after DeleteFileW: {error:?}"),
    };
    assert!(
        read_server_qualification_command(&control)
            .unwrap_or_else(|error| panic!("{regime} was a fatal poll error: {error:#}"))
            .is_none(),
        "{regime} must read as no command this tick"
    );
    drop(held);
}

#[cfg(unix)]
#[test]
fn qualification_gate_rejects_broad_or_linked_filesystem_surfaces() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let temporary = tempfile::tempdir().expect("temporary qualification root");
    let directory = temporary.path().join("qualification");
    fs::create_dir(&directory).expect("qualification directory");
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o755))
        .expect("set broad directory mode");
    let canonical = fs::canonicalize(&directory).expect("canonical qualification directory");
    let broad_error = server_qualification_control_from_values(
        Some(canonical.clone().into_os_string()),
        Some("test-nonce".into()),
    )
    .expect_err("group- or world-accessible qualification directories are rejected");
    assert!(
        broad_error
            .to_string()
            .contains("embedding_qualification_directory_untrusted")
    );

    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
        .expect("restore private directory mode");
    let linked_directory = temporary.path().join("linked-qualification");
    symlink(&canonical, &linked_directory).expect("link qualification directory");
    let linked_error = server_qualification_control_from_values(
        Some(linked_directory.into_os_string()),
        Some("test-nonce".into()),
    )
    .expect_err("linked qualification directories are rejected");
    assert!(
        linked_error
            .to_string()
            .contains("embedding_qualification_directory_untrusted")
    );

    let event_target = temporary.path().join("event-target");
    fs::write(&event_target, b"").expect("event target");
    fs::set_permissions(&event_target, fs::Permissions::from_mode(0o600))
        .expect("private event target");
    symlink(&event_target, canonical.join("test-nonce.events.jsonl")).expect("link event log");
    let event_error = server_qualification_control_from_values(
        Some(canonical.into_os_string()),
        Some("test-nonce".into()),
    )
    .expect_err("linked event logs are rejected");
    assert!(
        event_error
            .to_string()
            .contains("embedding_qualification_file_untrusted")
    );
}

#[cfg(unix)]
#[test]
fn qualification_gate_bounds_and_pins_commands_and_events() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let (temporary, control) = test_qualification_control();
    let command_path = control
        .directory
        .join(format!("{}.command.json", control.nonce));
    let command_target = temporary.path().join("command-target");
    fs::write(&command_target, b"{}").expect("command target");
    fs::set_permissions(&command_target, fs::Permissions::from_mode(0o600))
        .expect("private command target");
    symlink(&command_target, &command_path).expect("link command");
    assert!(
        read_server_qualification_command(&control)
            .expect_err("linked commands are rejected")
            .to_string()
            .contains("embedding_qualification_file_untrusted")
    );

    fs::remove_file(&command_path).expect("remove command link");
    fs::write(
        &command_path,
        vec![b'x'; SERVER_QUALIFICATION_MAX_COMMAND_BYTES as usize + 1],
    )
    .expect("oversized command");
    fs::set_permissions(&command_path, fs::Permissions::from_mode(0o600))
        .expect("private oversized command");
    assert!(
        read_server_qualification_command(&control)
            .expect_err("oversized commands are rejected")
            .to_string()
            .contains("embedding_qualification_file_untrusted")
    );

    fs::write(&command_path, b"{}").expect("bounded command");
    fs::set_permissions(&command_path, fs::Permissions::from_mode(0o600))
        .expect("private bounded command");
    let command = read_server_qualification_command(&control)
        .expect("read bounded command")
        .expect("command exists");
    let command_sha256 = hex_sha256(&command.bytes);
    control.mark_command_processed(command_sha256.clone());
    assert!(control.command_was_processed(&command_sha256));
    assert!(
        command_path.exists(),
        "the server leaves qualification command cleanup to its writer"
    );

    fs::remove_file(&command_path).expect("remove read command");
    fs::write(&command_path, b"{\"replacement\":true}").expect("replacement command");
    fs::set_permissions(&command_path, fs::Permissions::from_mode(0o600))
        .expect("private replacement command");
    let replacement = read_server_qualification_command(&control)
        .expect("read replacement command")
        .expect("replacement command exists");
    let replacement_sha256 = hex_sha256(&replacement.bytes);
    assert!(
        !control.command_was_processed(&replacement_sha256),
        "replacement content is never mistaken for the processed command"
    );
    assert!(
        command_path.exists(),
        "replacement command remains untouched"
    );

    let mut events = control.events.lock().expect("event log");
    events.records = SERVER_QUALIFICATION_MAX_EVENT_RECORDS;
    assert!(
        events
            .record(&control.directory, &test_qualification_event())
            .expect_err("event record count is bounded")
            .to_string()
            .contains("embedding_qualification_event_log_limit")
    );
    events.records = 0;
    events
        .file
        .set_len(SERVER_QUALIFICATION_MAX_EVENT_BYTES)
        .expect("expand event log to byte limit");
    events.bytes = SERVER_QUALIFICATION_MAX_EVENT_BYTES;
    assert!(
        events
            .record(&control.directory, &test_qualification_event())
            .expect_err("event bytes are bounded")
            .to_string()
            .contains("embedding_qualification_event_log_limit")
    );
    events.file.set_len(0).expect("reset event log");
    events.bytes = 0;
    let moved_event_path = events.path.with_extension("moved");
    fs::rename(&events.path, &moved_event_path).expect("move pinned event log");
    fs::write(&events.path, b"").expect("replacement event log");
    fs::set_permissions(&events.path, fs::Permissions::from_mode(0o600))
        .expect("private replacement event log");
    assert!(
        events
            .record(&control.directory, &test_qualification_event())
            .expect_err("replacement event logs are rejected")
            .to_string()
            .contains("embedding_qualification_event_log_replaced")
    );
    drop(events);

    let original_directory = control.directory.path.clone();
    let moved_directory = temporary.path().join("moved-qualification");
    fs::rename(&original_directory, &moved_directory).expect("move pinned directory");
    fs::create_dir(&original_directory).expect("replacement directory");
    fs::set_permissions(&original_directory, fs::Permissions::from_mode(0o700))
        .expect("private replacement directory");
    assert!(
        control
            .directory
            .revalidate()
            .expect_err("replacement directories are rejected")
            .to_string()
            .contains("embedding_qualification_directory_replaced")
    );
}

#[cfg(windows)]
#[test]
fn qualification_event_log_rejects_a_replaced_windows_path() {
    let (_temporary, control) = test_qualification_control();
    let mut events = control.events.lock().expect("event log");
    let moved_event_path = events.path.with_extension("moved");
    fs::rename(&events.path, &moved_event_path).expect("move pinned event log");
    fs::write(&events.path, b"").expect("replacement event log");

    assert!(
        events
            .record(&control.directory, &test_qualification_event())
            .expect_err("replacement event logs are rejected")
            .to_string()
            .contains("embedding_qualification_event_log_replaced")
    );
}

#[cfg(windows)]
#[test]
fn qualification_gate_accepts_native_identical_windows_path_spellings() {
    let temporary = tempfile::tempdir().expect("temporary qualification root");
    let directory = temporary.path().join("qualification");
    fs::create_dir(&directory).expect("qualification directory");
    let canonical = fs::canonicalize(&directory).expect("canonical qualification directory");
    assert_ne!(
        directory, canonical,
        "Windows canonicalization should expose the verbatim spelling mismatch"
    );
    assert_eq!(
        native_path_identity(&directory).expect("caller directory identity"),
        native_path_identity(&canonical).expect("canonical directory identity")
    );

    let control = server_qualification_control_from_values(
        Some(directory.into_os_string()),
        Some("test-nonce".into()),
    )
    .expect("native-identical Windows spellings are trusted")
    .expect("qualification control is enabled");

    assert_eq!(control.directory.path, canonical);
    control
        .directory
        .revalidate()
        .expect("canonical directory remains pinned");
}

#[cfg(unix)]
#[test]
fn qualification_restart_restores_the_last_durable_command_sequence() {
    let (_temporary, control) = test_qualification_control();
    let directory = control.directory.path.clone();
    let mut event = test_qualification_event();
    event.sequence = 7;
    control
        .events
        .lock()
        .expect("event log")
        .record(&control.directory, &event)
        .expect("durable qualification event");
    drop(control);

    let restarted = server_qualification_control_from_values(
        Some(directory.into_os_string()),
        Some("test-nonce".into()),
    )
    .expect("reopen qualification control")
    .expect("qualification control remains enabled");
    assert_eq!(restarted.last_sequence.load(Ordering::Acquire), 7);
}
