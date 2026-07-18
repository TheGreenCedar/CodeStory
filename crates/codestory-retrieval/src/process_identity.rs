#[cfg(any(
    test,
    windows,
    all(unix, not(any(target_os = "linux", target_os = "macos")))
))]
use anyhow::{Context, Result, bail};
#[cfg(target_os = "linux")]
use std::fs;
#[cfg(windows)]
use std::io;
#[cfg(windows)]
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};
#[cfg(target_os = "linux")]
use std::path::Path;
#[cfg(any(test, all(unix, not(any(target_os = "linux", target_os = "macos")))))]
use std::process::Command;
#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
use std::process::Output;
#[cfg(any(
    all(unix, not(any(target_os = "linux", target_os = "macos"))),
    all(test, windows)
))]
use std::process::Stdio;
use std::sync::OnceLock;
#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
use std::thread;
#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
use std::time::{Duration, Instant};

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
const PROCESS_PROBE_TIMEOUT: Duration = Duration::from_secs(2);
#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
const PROCESS_PROBE_REAP_TIMEOUT: Duration = Duration::from_millis(250);
#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
const PROCESS_PROBE_POLL: Duration = Duration::from_millis(10);

#[cfg(windows)]
const WINDOWS_PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
#[cfg(windows)]
const WINDOWS_ERROR_INVALID_PARAMETER: i32 = 87;
#[cfg(windows)]
const WINDOWS_STILL_ACTIVE: u32 = 259;
#[cfg(windows)]
const WINDOWS_DATETIME_TICKS_AT_FILETIME_EPOCH: u64 = 504_911_232_000_000_000;

/// Live-process evidence from the platform start-identity probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcessStartProbe {
    Running { start_identity: String },
    NotRunning,
    Unknown { reason: String },
}

/// Whether a process probe still proves ownership of a recorded PID.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessOwnerState {
    Matching,
    GoneOrReused,
    Unknown,
}

/// Compare a live process probe with an optional persisted start identity.
pub fn process_owner_state(
    probe: &ProcessStartProbe,
    expected_start_identity: Option<&str>,
) -> ProcessOwnerState {
    match probe {
        ProcessStartProbe::NotRunning => ProcessOwnerState::GoneOrReused,
        ProcessStartProbe::Unknown { .. } => ProcessOwnerState::Unknown,
        ProcessStartProbe::Running { start_identity } => match expected_start_identity {
            None => ProcessOwnerState::Matching,
            Some(expected) if start_identity == expected => ProcessOwnerState::Matching,
            Some(_) => ProcessOwnerState::GoneOrReused,
        },
    }
}

/// Probe the process start identity without collapsing unavailable evidence into death.
pub fn probe_process_start_identity(pid: u32) -> ProcessStartProbe {
    static CURRENT_PROCESS_START_IDENTITY: OnceLock<String> = OnceLock::new();

    if pid == std::process::id()
        && let Some(identity) = CURRENT_PROCESS_START_IDENTITY.get()
    {
        return ProcessStartProbe::Running {
            start_identity: identity.clone(),
        };
    }

    let probe = probe_process_start_identity_platform(pid);
    if pid == std::process::id()
        && let ProcessStartProbe::Running { start_identity } = &probe
    {
        let _ = CURRENT_PROCESS_START_IDENTITY.set(start_identity.clone());
    }
    probe
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
fn bounded_process_command_output(command: &mut Command) -> Result<Output> {
    bounded_process_command_output_with_timeout(command, PROCESS_PROBE_TIMEOUT)
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PsProbeOutputStatus {
    Success,
    ProcessMissing,
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
pub(crate) fn classify_ps_probe_output(output: &Output) -> Result<PsProbeOutputStatus> {
    if output.status.success() {
        return Ok(PsProbeOutputStatus::Success);
    }
    if output.status.code() == Some(1) {
        return Ok(PsProbeOutputStatus::ProcessMissing);
    }
    bail!(
        "ps exited with {}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr).trim()
    )
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
fn bounded_process_command_output_with_timeout(
    command: &mut Command,
    timeout: Duration,
) -> Result<Output> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("spawn process identity probe")?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                return child
                    .wait_with_output()
                    .context("collect process identity probe output");
            }
            Ok(None) if Instant::now() < deadline => thread::sleep(PROCESS_PROBE_POLL),
            Ok(None) => {
                terminate_bounded_probe(&mut child);
                bail!(
                    "process identity probe timed out after {}ms",
                    timeout.as_millis()
                );
            }
            Err(error) => {
                terminate_bounded_probe(&mut child);
                return Err(error).context("wait for process identity probe");
            }
        }
    }
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
fn terminate_bounded_probe(child: &mut std::process::Child) {
    let _ = child.kill();
    let reap_deadline = Instant::now() + PROCESS_PROBE_REAP_TIMEOUT;
    while Instant::now() < reap_deadline {
        if child.try_wait().ok().flatten().is_some() {
            break;
        }
        thread::sleep(PROCESS_PROBE_POLL);
    }
}

#[cfg(windows)]
#[repr(C)]
#[derive(Default)]
struct WindowsFileTime {
    low_date_time: u32,
    high_date_time: u32,
}

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn OpenProcess(desired_access: u32, inherit_handle: i32, process_id: u32) -> RawHandle;
    fn GetProcessTimes(
        process: RawHandle,
        creation_time: *mut WindowsFileTime,
        exit_time: *mut WindowsFileTime,
        kernel_time: *mut WindowsFileTime,
        user_time: *mut WindowsFileTime,
    ) -> i32;
    fn GetExitCodeProcess(process: RawHandle, exit_code: *mut u32) -> i32;
}

#[cfg(windows)]
fn windows_running_process_creation_time(pid: u32) -> Result<Option<WindowsFileTime>> {
    if pid == 0 {
        bail!("process pid must be greater than zero");
    }
    let raw_handle = unsafe { OpenProcess(WINDOWS_PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if raw_handle.is_null() {
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(WINDOWS_ERROR_INVALID_PARAMETER) {
            return Ok(None);
        }
        return Err(error).with_context(|| format!("open process {pid}"));
    }
    let process = unsafe { OwnedHandle::from_raw_handle(raw_handle) };
    let mut creation_time = WindowsFileTime::default();
    let mut exit_time = WindowsFileTime::default();
    let mut kernel_time = WindowsFileTime::default();
    let mut user_time = WindowsFileTime::default();
    if unsafe {
        GetProcessTimes(
            process.as_raw_handle(),
            &mut creation_time,
            &mut exit_time,
            &mut kernel_time,
            &mut user_time,
        )
    } == 0
    {
        return Err(io::Error::last_os_error())
            .with_context(|| format!("query process start identity for pid {pid}"));
    }
    let mut exit_code = 0_u32;
    if unsafe { GetExitCodeProcess(process.as_raw_handle(), &mut exit_code) } == 0 {
        return Err(io::Error::last_os_error())
            .with_context(|| format!("query process exit code for pid {pid}"));
    }
    if !windows_process_is_running(&exit_time, exit_code) {
        return Ok(None);
    }
    Ok(Some(creation_time))
}

#[cfg(windows)]
fn windows_filetime_ticks(filetime: &WindowsFileTime) -> u64 {
    (u64::from(filetime.high_date_time) << 32) | u64::from(filetime.low_date_time)
}

#[cfg(windows)]
fn windows_process_is_running(exit_time: &WindowsFileTime, exit_code: u32) -> bool {
    windows_filetime_ticks(exit_time) == 0 && exit_code == WINDOWS_STILL_ACTIVE
}

#[cfg(windows)]
fn windows_datetime_ticks_from_filetime(filetime: &WindowsFileTime) -> Result<u64> {
    let filetime_ticks = windows_filetime_ticks(filetime);
    // Win32_Process.CreationDate exposes microseconds, so discard sub-microsecond
    // FILETIME ticks to preserve identities serialized by the previous CIM query.
    let legacy_filetime_ticks = filetime_ticks / 10 * 10;
    legacy_filetime_ticks
        .checked_add(WINDOWS_DATETIME_TICKS_AT_FILETIME_EPOCH)
        .context("convert Windows process creation time to DateTime ticks")
}

#[cfg(windows)]
fn probe_process_start_identity_platform(pid: u32) -> ProcessStartProbe {
    match windows_running_process_creation_time(pid) {
        Ok(Some(creation_time)) => match windows_datetime_ticks_from_filetime(&creation_time) {
            Ok(ticks) => ProcessStartProbe::Running {
                start_identity: format!("windows:{ticks}"),
            },
            Err(error) => ProcessStartProbe::Unknown {
                reason: error.to_string(),
            },
        },
        Ok(None) => ProcessStartProbe::NotRunning,
        Err(error) => ProcessStartProbe::Unknown {
            reason: error.to_string(),
        },
    }
}

#[cfg(target_os = "linux")]
fn probe_process_start_identity_platform(pid: u32) -> ProcessStartProbe {
    let stat_path = Path::new("/proc").join(pid.to_string()).join("stat");
    let stat = match fs::read_to_string(&stat_path) {
        Ok(stat) => stat,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return ProcessStartProbe::NotRunning;
        }
        Err(error) => {
            return ProcessStartProbe::Unknown {
                reason: format!("read {}: {error}", stat_path.display()),
            };
        }
    };
    let Some(start_ticks) = stat
        .rsplit_once(") ")
        .and_then(|(_, fields)| fields.split_whitespace().nth(19))
    else {
        return ProcessStartProbe::Unknown {
            reason: format!("parse process start identity from {}", stat_path.display()),
        };
    };
    ProcessStartProbe::Running {
        start_identity: format!("linux:{start_ticks}"),
    }
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
fn probe_process_start_identity_platform(pid: u32) -> ProcessStartProbe {
    let mut command = Command::new("ps");
    command
        .env("LC_ALL", "C")
        .env("TZ", "UTC")
        .args(["-p", &pid.to_string(), "-o", "lstart="]);
    let output = match bounded_process_command_output(&mut command) {
        Ok(output) => output,
        Err(error) => {
            return ProcessStartProbe::Unknown {
                reason: error.to_string(),
            };
        }
    };
    match classify_ps_probe_output(&output) {
        Ok(PsProbeOutputStatus::Success) => {}
        Ok(PsProbeOutputStatus::ProcessMissing) => return ProcessStartProbe::NotRunning,
        Err(error) => {
            return ProcessStartProbe::Unknown {
                reason: error.to_string(),
            };
        }
    }
    let identity = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if identity.is_empty() {
        return ProcessStartProbe::Unknown {
            reason: "ps returned an empty process start identity".to_string(),
        };
    }
    ProcessStartProbe::Running {
        start_identity: format!("unix:{identity}"),
    }
}

#[cfg(target_os = "macos")]
fn probe_process_start_identity_platform(pid: u32) -> ProcessStartProbe {
    let mut info = unsafe { std::mem::zeroed::<libc::proc_bsdinfo>() };
    let expected = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
    let read = unsafe {
        libc::proc_pidinfo(
            pid as i32,
            libc::PROC_PIDTBSDINFO,
            0,
            (&mut info as *mut libc::proc_bsdinfo).cast(),
            expected,
        )
    };
    if read == 0 {
        let error = std::io::Error::last_os_error();
        return match error.raw_os_error() {
            Some(libc::ESRCH) | Some(libc::ENOENT) => ProcessStartProbe::NotRunning,
            _ => ProcessStartProbe::Unknown {
                reason: error.to_string(),
            },
        };
    }
    if read != expected || info.pbi_pid != pid {
        return ProcessStartProbe::Unknown {
            reason: "macOS process identity was incomplete".into(),
        };
    }
    ProcessStartProbe::Running {
        start_identity: format!(
            "macos-proc:{}:{}",
            info.pbi_start_tvsec, info.pbi_start_tvusec
        ),
    }
}

#[cfg(not(any(windows, unix)))]
fn probe_process_start_identity_platform(_pid: u32) -> ProcessStartProbe {
    ProcessStartProbe::Unknown {
        reason: "process start identity is unsupported on this platform".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_owner_state_preserves_identity_and_probe_uncertainty() {
        let running = ProcessStartProbe::Running {
            start_identity: "start-a".to_string(),
        };
        assert_eq!(
            process_owner_state(&running, Some("start-a")),
            ProcessOwnerState::Matching
        );
        assert_eq!(
            process_owner_state(&running, Some("start-b")),
            ProcessOwnerState::GoneOrReused
        );
        assert_eq!(
            process_owner_state(&ProcessStartProbe::NotRunning, Some("start-a")),
            ProcessOwnerState::GoneOrReused
        );
        assert_eq!(
            process_owner_state(
                &ProcessStartProbe::Unknown {
                    reason: "probe failed".to_string(),
                },
                Some("start-a")
            ),
            ProcessOwnerState::Unknown
        );
        assert_eq!(
            process_owner_state(&running, None),
            ProcessOwnerState::Matching
        );
    }

    #[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
    #[test]
    fn ps_status_distinguishes_missing_process_from_probe_failure() {
        use std::os::unix::process::ExitStatusExt;

        let output = |code, stderr: &str| Output {
            status: std::process::ExitStatus::from_raw(code << 8),
            stdout: Vec::new(),
            stderr: stderr.as_bytes().to_vec(),
        };

        assert_eq!(
            classify_ps_probe_output(&output(0, "")).expect("successful ps"),
            PsProbeOutputStatus::Success
        );
        assert_eq!(
            classify_ps_probe_output(&output(1, "")).expect("missing pid"),
            PsProbeOutputStatus::ProcessMissing
        );
        let error = classify_ps_probe_output(&output(2, "invalid ps invocation"))
            .expect_err("probe failure must not look like a dead process");
        assert!(error.to_string().contains("invalid ps invocation"));
    }

    #[cfg(unix)]
    #[test]
    fn process_start_probe_tracks_live_child_and_exit() -> Result<()> {
        let mut child = Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .context("spawn process identity fixture")?;
        let pid = child.id();
        let expected_prefix = if cfg!(target_os = "linux") {
            "linux:"
        } else if cfg!(target_os = "macos") {
            "macos-proc:"
        } else {
            "unix:"
        };

        let first = probe_process_start_identity(pid);
        let second = probe_process_start_identity(pid);
        let result = (|| -> Result<()> {
            let ProcessStartProbe::Running {
                start_identity: first,
            } = first
            else {
                bail!("live child did not expose a process start identity")
            };
            let ProcessStartProbe::Running {
                start_identity: second,
            } = second
            else {
                bail!("repeat live-child probe did not expose a process start identity")
            };
            assert!(first.starts_with(expected_prefix));
            assert_eq!(first, second);
            Ok(())
        })();

        let _ = child.kill();
        let _ = child.wait();
        result?;
        assert_eq!(
            probe_process_start_identity(pid),
            ProcessStartProbe::NotRunning
        );
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn windows_identity_format_stays_compatible_with_legacy_cim_ticks() -> Result<()> {
        const DOTNET_DATETIME_TICKS_AT_UNIX_EPOCH: u64 = 621_355_968_000_000_000;

        const WINDOWS_FILETIME_TICKS_AT_UNIX_EPOCH: u64 = 116_444_736_000_000_000;
        let unix_epoch_with_sub_microsecond_ticks = WINDOWS_FILETIME_TICKS_AT_UNIX_EPOCH + 8;
        let unix_epoch = WindowsFileTime {
            low_date_time: unix_epoch_with_sub_microsecond_ticks as u32,
            high_date_time: (unix_epoch_with_sub_microsecond_ticks >> 32) as u32,
        };
        assert_eq!(
            windows_datetime_ticks_from_filetime(&unix_epoch)?,
            DOTNET_DATETIME_TICKS_AT_UNIX_EPOCH
        );
        assert!(windows_process_is_running(
            &WindowsFileTime::default(),
            WINDOWS_STILL_ACTIVE
        ));
        assert!(!windows_process_is_running(
            &WindowsFileTime {
                low_date_time: 1,
                high_date_time: 0,
            },
            WINDOWS_STILL_ACTIVE
        ));
        assert!(!windows_process_is_running(&WindowsFileTime::default(), 0));
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn windows_process_start_probe_rejects_exited_child() -> Result<()> {
        let mut child = Command::new("ping")
            .args(["-n", "30", "127.0.0.1"])
            .stdout(Stdio::null())
            .spawn()
            .context("spawn Windows process identity fixture")?;
        let pid = child.id();
        let result = match probe_process_start_identity(pid) {
            ProcessStartProbe::Running { start_identity }
                if start_identity.starts_with("windows:") =>
            {
                Ok(())
            }
            probe => Err(anyhow::anyhow!(
                "live Windows child did not expose a process start identity: {probe:?}"
            )),
        };

        let _ = child.kill();
        let _ = child.wait();
        result?;
        assert_eq!(
            probe_process_start_identity(pid),
            ProcessStartProbe::NotRunning
        );
        Ok(())
    }
}
