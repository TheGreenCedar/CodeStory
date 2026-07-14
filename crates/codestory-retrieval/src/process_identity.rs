use anyhow::{Context, Result, bail};
#[cfg(target_os = "linux")]
use std::fs;
#[cfg(windows)]
use std::io;
#[cfg(windows)]
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};
#[cfg(target_os = "linux")]
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant};

const PROCESS_PROBE_TIMEOUT: Duration = Duration::from_secs(2);
const PROCESS_PROBE_REAP_TIMEOUT: Duration = Duration::from_millis(250);
const PROCESS_PROBE_POLL: Duration = Duration::from_millis(10);

#[cfg(windows)]
const WINDOWS_PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
#[cfg(windows)]
const WINDOWS_ERROR_INVALID_PARAMETER: i32 = 87;
#[cfg(windows)]
const WINDOWS_DATETIME_TICKS_AT_FILETIME_EPOCH: u64 = 504_911_232_000_000_000;
#[cfg(windows)]
const WINDOWS_FILETIME_TICKS_AT_UNIX_EPOCH: u64 = 116_444_736_000_000_000;

/// Live-process evidence from the platform start-identity probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcessStartProbe {
    Running { start_identity: Option<String> },
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
            Some(expected) => match start_identity {
                Some(actual) if actual == expected => ProcessOwnerState::Matching,
                Some(_) => ProcessOwnerState::GoneOrReused,
                None => ProcessOwnerState::Unknown,
            },
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
            start_identity: Some(identity.clone()),
        };
    }

    let probe = probe_process_start_identity_platform(pid);
    if pid == std::process::id()
        && let ProcessStartProbe::Running {
            start_identity: Some(identity),
        } = &probe
    {
        let _ = CURRENT_PROCESS_START_IDENTITY.set(identity.clone());
    }
    probe
}

/// Return the persisted native embedding identity format used by sidecar state.
pub fn native_embedding_process_start_identity(pid: u32) -> Result<Option<String>> {
    if pid == 0 {
        bail!("native embedding process pid must be greater than zero");
    }
    match probe_process_start_identity(pid) {
        ProcessStartProbe::Running {
            start_identity: Some(identity),
        } => Ok(Some(identity)),
        ProcessStartProbe::Running {
            start_identity: None,
        } => bail!("native embedding process start identity is unavailable for pid {pid}"),
        ProcessStartProbe::NotRunning => Ok(None),
        ProcessStartProbe::Unknown { reason } => {
            bail!("query native embedding start identity for pid {pid}: {reason}")
        }
    }
}

pub(crate) fn bounded_process_command_output(command: &mut Command) -> Result<Output> {
    bounded_process_command_output_with_timeout(command, PROCESS_PROBE_TIMEOUT)
}

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
}

#[cfg(windows)]
fn windows_process_creation_time(pid: u32) -> Result<Option<WindowsFileTime>> {
    if pid == 0 {
        bail!("native embedding process pid must be greater than zero");
    }
    let raw_handle = unsafe { OpenProcess(WINDOWS_PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if raw_handle.is_null() {
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(WINDOWS_ERROR_INVALID_PARAMETER) {
            return Ok(None);
        }
        return Err(error).with_context(|| format!("open native embedding process {pid}"));
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
            .with_context(|| format!("query native embedding start identity for pid {pid}"));
    }
    Ok(Some(creation_time))
}

#[cfg(windows)]
fn windows_filetime_ticks(filetime: &WindowsFileTime) -> u64 {
    (u64::from(filetime.high_date_time) << 32) | u64::from(filetime.low_date_time)
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
fn windows_epoch_ms_from_filetime(filetime: &WindowsFileTime) -> Result<i64> {
    let elapsed_ticks = windows_filetime_ticks(filetime)
        .checked_sub(WINDOWS_FILETIME_TICKS_AT_UNIX_EPOCH)
        .context("convert Windows process creation time to Unix epoch")?;
    i64::try_from(elapsed_ticks / 10_000)
        .context("convert Windows process creation time to epoch milliseconds")
}

#[cfg(windows)]
pub(crate) fn process_started_at_epoch_ms(pid: u32) -> Result<Option<i64>> {
    windows_process_creation_time(pid)?
        .as_ref()
        .map(windows_epoch_ms_from_filetime)
        .transpose()
}

#[cfg(not(windows))]
pub(crate) fn process_started_at_epoch_ms(pid: u32) -> Result<Option<i64>> {
    let mut command = Command::new("ps");
    command
        .env("LC_ALL", "C")
        .env("TZ", "UTC")
        .args(["-p", &pid.to_string(), "-o", "lstart="]);
    let output = bounded_process_command_output(&mut command)?;
    if !output.status.success() {
        return Ok(None);
    }
    Ok(process_started_at_epoch_ms_from_lstart(&output.stdout))
}

#[cfg(not(windows))]
fn process_started_at_epoch_ms_from_lstart(output: &[u8]) -> Option<i64> {
    use chrono::TimeZone;

    let started = chrono::NaiveDateTime::parse_from_str(
        String::from_utf8_lossy(output).trim(),
        "%a %b %e %H:%M:%S %Y",
    )
    .ok()?;
    Some(chrono::Utc.from_utc_datetime(&started).timestamp_millis())
}

#[cfg(windows)]
fn probe_process_start_identity_platform(pid: u32) -> ProcessStartProbe {
    match windows_process_creation_time(pid) {
        Ok(Some(creation_time)) => match windows_datetime_ticks_from_filetime(&creation_time) {
            Ok(ticks) => ProcessStartProbe::Running {
                start_identity: Some(format!("windows:{ticks}")),
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
        start_identity: Some(format!("linux:{start_ticks}")),
    }
}

#[cfg(all(unix, not(target_os = "linux")))]
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
    if !output.status.success() {
        return if output.status.code() == Some(1) {
            ProcessStartProbe::NotRunning
        } else {
            ProcessStartProbe::Unknown {
                reason: format!(
                    "ps exited with {}: {}",
                    output.status,
                    String::from_utf8_lossy(&output.stderr).trim()
                ),
            }
        };
    }
    let identity = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if identity.is_empty() {
        return ProcessStartProbe::Unknown {
            reason: "ps returned an empty process start identity".to_string(),
        };
    }
    ProcessStartProbe::Running {
        start_identity: Some(format!("unix:{identity}")),
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
            start_identity: Some("start-a".to_string()),
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
            process_owner_state(
                &ProcessStartProbe::Running {
                    start_identity: None,
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
        } else {
            "unix:"
        };

        let first = probe_process_start_identity(pid);
        let second = probe_process_start_identity(pid);
        let result = (|| -> Result<()> {
            let ProcessStartProbe::Running {
                start_identity: Some(first),
            } = first
            else {
                bail!("live child did not expose a process start identity")
            };
            let ProcessStartProbe::Running {
                start_identity: Some(second),
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

        let unix_epoch_with_sub_microsecond_ticks = WINDOWS_FILETIME_TICKS_AT_UNIX_EPOCH + 8;
        let unix_epoch = WindowsFileTime {
            low_date_time: unix_epoch_with_sub_microsecond_ticks as u32,
            high_date_time: (unix_epoch_with_sub_microsecond_ticks >> 32) as u32,
        };
        assert_eq!(
            windows_datetime_ticks_from_filetime(&unix_epoch)?,
            DOTNET_DATETIME_TICKS_AT_UNIX_EPOCH
        );
        assert_eq!(windows_epoch_ms_from_filetime(&unix_epoch)?, 0);
        Ok(())
    }
}
