use super::super::{
    SERVER_QUALIFICATION_MAX_COMMAND_BYTES, SERVER_QUALIFICATION_MAX_EVENT_BYTES,
    SERVER_QUALIFICATION_MAX_EVENT_RECORDS, hex_sha256, read_server_qualification_command,
    server_qualification_control_from_values,
};
use super::{test_qualification_control, test_qualification_event};
use std::fs;
use std::sync::atomic::Ordering;

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
