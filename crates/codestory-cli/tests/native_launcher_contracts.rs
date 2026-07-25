mod test_support;

#[cfg(any(target_os = "linux", target_os = "windows"))]
use fs4::fs_std::FileExt;
#[cfg(any(target_os = "linux", target_os = "windows"))]
use std::fs::OpenOptions;
#[cfg(any(target_os = "linux", target_os = "windows"))]
use std::thread;
#[cfg(any(target_os = "linux", target_os = "windows"))]
use std::time::{Duration, Instant};

#[cfg(any(target_os = "linux", target_os = "windows"))]
#[test]
fn functional_cli_stays_available_while_launcher_activation_is_locked() {
    let launcher = test_support::launcher_binary_path();
    let lock_path = launcher
        .parent()
        .expect("launcher has a parent directory")
        .join(".codestory-native-staging.lock");
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)
        .expect("open native launcher staging lock");
    FileExt::lock_exclusive(&lock).expect("hold native launcher staging lock");

    let mut command = test_support::cli_command();
    let mut child = command
        .arg("--version")
        .spawn()
        .expect("spawn functional CLI");
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll functional CLI") {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("functional CLI blocked on the native launcher staging lock");
        }
        thread::sleep(Duration::from_millis(20));
    };
    assert!(status.success(), "functional CLI failed with {status}");
}
