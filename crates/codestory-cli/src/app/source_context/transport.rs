//! Once-through Unix rg tee and outer capture. No source reads or shell parser.
use super::{Payload, render, rows};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, RawFd};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

const FD: &str = "CODESTORY_SOURCE_CONTEXT_FD";
const PIPE_ID: &str = "CODESTORY_SOURCE_CONTEXT_PIPE_ID";
const REAL: &str = "CODESTORY_SOURCE_CONTEXT_BOUND_RG";
const ORIGINAL_PATH: &str = "CODESTORY_SOURCE_CONTEXT_ORIGINAL_PATH";
const CLI: &str = "CODESTORY_SOURCE_CONTEXT_CLI";
const IPC_MAX: usize = 262144;
const CHILD_MAX: usize = 65536;

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}
fn native(path: &Path) -> Option<[u64; 2]> {
    let m = std::fs::metadata(path).ok()?;
    (m.is_file() && m.permissions().mode() & 0o111 != 0).then_some([m.dev(), m.ino()])
}
fn nonblocking(fd: RawFd) -> std::io::Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}
fn pipe() -> std::io::Result<(File, File)> {
    let mut descriptors = [-1; 2];
    // SAFETY: pipe initializes two owned descriptors on success.
    if unsafe { libc::pipe(descriptors.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let pair = unsafe {
        (
            File::from_raw_fd(descriptors[0]),
            File::from_raw_fd(descriptors[1]),
        )
    };
    for file in [&pair.0, &pair.1] {
        nonblocking(file.as_raw_fd())?;
        if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(pair)
}

fn pipe_identity(fd: RawFd) -> Option<String> {
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    if unsafe { libc::fstat(fd, stat.as_mut_ptr()) } != 0 {
        return None;
    }
    let stat = unsafe { stat.assume_init() };
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0
        || stat.st_mode & libc::S_IFMT != libc::S_IFIFO
        || stat.st_ino == 0
        || flags & libc::O_ACCMODE != libc::O_WRONLY
    {
        return None;
    }
    Some(format!(
        "{}:{}:{}:{}",
        stat.st_dev,
        stat.st_ino,
        stat.st_mode & libc::S_IFMT,
        flags & (libc::O_ACCMODE | libc::O_NONBLOCK)
    ))
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Frame {
    Setup {
        id: String,
        real: String,
        native: [u64; 2],
    },
    Start {
        id: String,
        real: String,
        native: [u64; 2],
        argv: Vec<String>,
        cwd: String,
        real_pid: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        config_path_absent: Option<bool>,
    },
    Data {
        id: String,
        bytes: String,
    },
    End {
        id: String,
        code: i32,
        overflow: bool,
        complete: bool,
    },
}

struct Sender {
    fd: RawFd,
    maximum: usize,
    enabled: bool,
}
impl Sender {
    fn inherited() -> Option<Self> {
        let fd = std::env::var(FD).ok()?.parse::<RawFd>().ok()?;
        if fd < 3 {
            return None;
        }
        let expected = std::env::var(PIPE_ID).ok()?;
        Self::checked(fd, &expected)
    }
    fn checked(fd: RawFd, expected: &str) -> Option<Self> {
        if fd < 3 {
            return None;
        }
        if pipe_identity(fd).as_deref() != Some(expected) {
            return None;
        }
        let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
        if unsafe { libc::fstat(fd, stat.as_mut_ptr()) } != 0 {
            return None;
        }
        if unsafe { stat.assume_init() }.st_mode & libc::S_IFMT != libc::S_IFIFO {
            return None;
        }
        nonblocking(fd).ok()?;
        let maximum = unsafe { libc::fpathconf(fd, libc::_PC_PIPE_BUF) };
        if maximum <= 0 {
            return None;
        }
        Some(Self {
            fd,
            maximum: (maximum as usize).min(4096),
            enabled: true,
        })
    }
    fn send(&mut self, frame: &Frame) {
        if !self.enabled {
            return;
        }
        let Ok(mut raw) = serde_json::to_vec(frame) else {
            self.enabled = false;
            return;
        };
        raw.push(b'\n');
        if raw.len() > self.maximum {
            self.enabled = false;
            return;
        }
        // One nonblocking write <= PIPE_BUF is atomic, so parallel frames cannot
        // interleave. Lost/EAGAIN/oversize records simply cannot complete.
        let written = unsafe { libc::write(self.fd, raw.as_ptr().cast(), raw.len()) };
        if written != raw.len() as isize {
            self.enabled = false;
        }
    }
}

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
fn encode(raw: &[u8]) -> String {
    let mut out = String::with_capacity(raw.len().div_ceil(3) * 4);
    for chunk in raw.chunks(3) {
        let bits = ((chunk[0] as u32) << 16)
            | ((chunk.get(1).copied().unwrap_or(0) as u32) << 8)
            | chunk.get(2).copied().unwrap_or(0) as u32;
        out.push(ALPHABET[((bits >> 18) & 63) as usize] as char);
        out.push(ALPHABET[((bits >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[((bits >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[(bits & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}
fn decode(value: &str) -> Option<Vec<u8>> {
    if value.len() % 4 != 0 {
        return None;
    }
    let mut out = Vec::new();
    for chunk in value.as_bytes().chunks_exact(4) {
        let digit = |byte| ALPHABET.iter().position(|&v| v == byte).map(|v| v as u32);
        let a = digit(chunk[0])?;
        let b = digit(chunk[1])?;
        let c = if chunk[2] == b'=' {
            0
        } else {
            digit(chunk[2])?
        };
        let d = if chunk[3] == b'=' {
            0
        } else {
            digit(chunk[3])?
        };
        let bits = (a << 18) | (b << 12) | (c << 6) | d;
        out.push((bits >> 16) as u8);
        if chunk[2] != b'=' {
            out.push((bits >> 8) as u8);
        }
        if chunk[3] != b'=' {
            out.push(bits as u8);
        }
    }
    (encode(&out) == value).then_some(out)
}

pub(super) struct Record {
    pub argv: Vec<String>,
    pub cwd: String,
    pub stdout: Vec<u8>,
    pub code: i32,
    pub overflow: bool,
    pub config_path_absent: Option<bool>,
}
struct Pending {
    record: Record,
    done: bool,
}
struct PrivateRows {
    total: usize,
    pending: Vec<u8>,
    bad: bool,
    setup: Option<(String, [u64; 2])>,
    children: Vec<Pending>,
    ids: HashMap<String, usize>,
}
impl PrivateRows {
    fn new() -> Self {
        Self {
            total: 0,
            pending: Vec::new(),
            bad: false,
            setup: None,
            children: Vec::new(),
            ids: HashMap::new(),
        }
    }
    fn push(&mut self, raw: &[u8]) {
        self.total = self.total.saturating_add(raw.len());
        if self.total > IPC_MAX {
            self.bad = true;
        }
        if self.bad {
            self.pending.clear();
            self.children.clear();
            return;
        }
        self.pending.extend_from_slice(raw);
        let mut consumed = 0;
        while let Some(length) = self.pending[consumed..]
            .iter()
            .position(|&byte| byte == b'\n')
        {
            let end = consumed + length;
            let frame = if length <= 4096 {
                serde_json::from_slice::<Frame>(&self.pending[consumed..end]).ok()
            } else {
                None
            };
            consumed = end + 1;
            if !frame.is_some_and(|frame| self.accept(frame)) {
                self.bad = true;
                break;
            }
        }
        self.pending.drain(..consumed);
        if self.pending.len() > 4096 {
            self.bad = true;
        }
        if self.bad {
            self.pending.clear();
            self.children.clear();
        }
    }
    fn accept(&mut self, frame: Frame) -> bool {
        match frame {
            Frame::Setup { id, real, native } => {
                if id != "setup" || self.setup.is_some() || !Path::new(&real).is_absolute() {
                    return false;
                }
                self.setup = Some((real, native));
            }
            Frame::Start {
                id,
                real,
                native,
                argv,
                cwd,
                real_pid,
                config_path_absent,
            } => {
                if id.is_empty()
                    || id.len() > 32
                    || self.ids.contains_key(&id)
                    || self.children.len() >= 32
                    || real_pid == 0
                    || self.setup.as_ref() != Some(&(real, native))
                    || !Path::new(&cwd).is_absolute()
                {
                    return false;
                }
                self.ids.insert(id, self.children.len());
                self.children.push(Pending {
                    record: Record {
                        argv,
                        cwd,
                        stdout: Vec::new(),
                        code: -1,
                        overflow: false,
                        config_path_absent,
                    },
                    done: false,
                });
            }
            Frame::Data { id, bytes } => {
                let Some(&index) = self.ids.get(&id) else {
                    return false;
                };
                let child = &mut self.children[index];
                let Some(bytes) = decode(&bytes) else {
                    return false;
                };
                if child.done || child.record.stdout.len() + bytes.len() > CHILD_MAX {
                    return false;
                }
                child.record.stdout.extend_from_slice(&bytes);
            }
            Frame::End {
                id,
                code,
                overflow,
                complete,
            } => {
                let Some(&index) = self.ids.get(&id) else {
                    return false;
                };
                let child = &mut self.children[index];
                if child.done || !complete {
                    return false;
                }
                child.done = true;
                child.record.code = code;
                child.record.overflow = overflow;
            }
        }
        true
    }
    fn records(self) -> Vec<Record> {
        if self.bad || !self.pending.is_empty() {
            return Vec::new();
        }
        self.children
            .into_iter()
            .filter(|child| child.done)
            .map(|child| child.record)
            .collect()
    }
}

pub(super) fn setup(encoded: &str, real: &Path, original_path: &OsStr) -> anyhow::Result<()> {
    let Some(_payload) = Payload::decode(encoded) else {
        return Ok(());
    };
    let (Some(real_text), Some(original)) = (real.to_str(), original_path.to_str()) else {
        return Ok(());
    };
    if !real.is_absolute() {
        return Ok(());
    }
    let Some(native) = native(real) else {
        return Ok(());
    };
    if let Some(mut sender) = Sender::inherited() {
        sender.send(&Frame::Setup {
            id: "setup".into(),
            real: real_text.to_owned(),
            native,
        });
    }
    println!(
        "export {REAL}={}\nexport {ORIGINAL_PATH}={}\nexport CODESTORY_SOURCE_CONTEXT_READY=1",
        quote(real_text),
        quote(original)
    );
    Ok(())
}

fn finish(status: ExitStatus) -> ! {
    if let Some(signal) = status.signal() {
        unsafe {
            libc::signal(signal, libc::SIG_DFL);
            libc::raise(signal);
        }
        std::process::exit(128 + signal);
    }
    std::process::exit(status.code().unwrap_or(1));
}

pub(super) fn rg(operands: Vec<OsString>) -> anyhow::Result<()> {
    let real = std::env::var_os(REAL).ok_or_else(|| anyhow::anyhow!("missing bound rg"))?;
    if !Path::new(&real).is_absolute() {
        anyhow::bail!("bound rg must be absolute");
    }
    let mut command = Command::new(&real);
    command.args(&operands);
    if let Some(path) = std::env::var_os(ORIGINAL_PATH) {
        command.env("PATH", path);
    }
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    let piped = unsafe { libc::fstat(1, stat.as_mut_ptr()) } == 0
        && unsafe { stat.assume_init() }.st_mode & libc::S_IFMT == libc::S_IFIFO;
    let Some(mut sender) = piped.then(Sender::inherited).flatten() else {
        return Err(command.exec().into());
    };
    // Real rg and its descendants do not need the observation writer.
    unsafe {
        libc::fcntl(sender.fd, libc::F_SETFD, libc::FD_CLOEXEC);
    }
    let config_path_absent = config_absence_at_spawn();
    let mut child = command.stdout(Stdio::piped()).spawn()?;
    let id = std::process::id().to_string();
    let actual = operands
        .iter()
        .map(|arg| arg.to_str().map(str::to_owned))
        .collect::<Option<Vec<_>>>();
    let cwd = std::env::current_dir()
        .ok()
        .and_then(|p| p.to_str().map(str::to_owned));
    if let (Some(argv), Some(cwd), Some(native), Some(real)) =
        (actual, cwd, native(Path::new(&real)), real.to_str())
    {
        let maximum = sender.maximum;
        sender.send(&bounded_start(
            Frame::Start {
                id: id.clone(),
                real: real.to_owned(),
                native,
                argv,
                cwd,
                real_pid: child.id(),
                config_path_absent,
            },
            maximum,
        ));
    } else {
        sender.enabled = false;
    }
    let mut input = child.stdout.take().unwrap();
    let mut output = std::io::stdout().lock();
    let mut buffer = [0u8; 65536];
    let mut captured = 0usize;
    let mut overflow = false;
    loop {
        let count = match input.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => {
                overflow = true;
                break;
            }
        };
        if output.write_all(&buffer[..count]).is_err() {
            overflow = true;
            break;
        }
        if captured.saturating_add(count) > CHILD_MAX {
            overflow = true;
        }
        if !overflow {
            for chunk in buffer[..count].chunks(192) {
                sender.send(&Frame::Data {
                    id: id.clone(),
                    bytes: encode(chunk),
                });
            }
            captured += count;
        }
    }
    drop(input);
    let _ = output.flush();
    let status = child.wait()?;
    sender.send(&Frame::End {
        id,
        code: status
            .code()
            .unwrap_or_else(|| -status.signal().unwrap_or(1)),
        overflow,
        complete: sender.enabled,
    });
    finish(status)
}

fn config_absence_at_spawn() -> Option<bool> {
    // This process is the actual rg child boundary. Capture only absence, never
    // the config path/value, before spawning the command with this environment.
    Some(std::env::var_os("RIPGREP_CONFIG_PATH").is_none())
}

fn bounded_start(mut frame: Frame, maximum: usize) -> Frame {
    if serde_json::to_vec(&frame).is_ok_and(|raw| raw.len().saturating_add(1) > maximum) {
        if let Frame::Start {
            config_path_absent, ..
        } = &mut frame
        {
            // Optional features must not make a previously admissible atomic
            // observation disappear. Unknown config retains ordinary behavior.
            *config_path_absent = None;
        }
    }
    frame
}

pub(super) fn capture(payload: &Payload) -> anyhow::Result<()> {
    let (private, writer) = pipe()?;
    let writer_fd = writer.as_raw_fd();
    let cli = std::env::current_exe()?;
    let cli_text = cli
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("unsupported executable path"))?;
    let encoded = serde_json::to_vec(payload)?
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let shim = payload
        .shim_directory
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("unsupported shim path"))?;
    let asset = std::fs::symlink_metadata(payload.shim_directory.join("rg"))
        .ok()
        .is_some_and(|m| {
            m.is_file() && !m.file_type().is_symlink() && m.permissions().mode() & 0o111 != 0
        });
    // Fixed owned prelude runs AFTER the same actual login startup. The original
    // command is a literal contiguous suffix and is never parsed or replayed.
    let prelude = if asset {
        format!(
            "unset CODESTORY_SOURCE_CONTEXT_READY\nexport {REAL}=\"$(builtin whence -p rg)\"\nif [[ \"$CODESTORY_SOURCE_CONTEXT_BOUND_RG\" = /* && -x \"$CODESTORY_SOURCE_CONTEXT_BOUND_RG\" ]]; then\nexport {CLI}={}\neval \"$({} internal-source-context setup --payload {} --rg \"$CODESTORY_SOURCE_CONTEXT_BOUND_RG\" --original-path \"$PATH\")\"\nif [[ \"$CODESTORY_SOURCE_CONTEXT_READY\" = 1 ]]; then\nexport PATH={}:\"$PATH\"\nrehash\nfi\nfi\n",
            quote(cli_text),
            quote(cli_text),
            encoded,
            quote(shim)
        )
    } else {
        String::new()
    };
    let mut command = Command::new("/bin/zsh");
    command
        .arg("-lc")
        .arg(format!("{prelude}{}", payload.command))
        .current_dir(&payload.cwd)
        .env(FD, writer_fd.to_string())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // SAFETY: only async-signal-safe fcntl runs after fork; descriptor ownership
    // remains in the parent until spawn completes. No process group is detached.
    unsafe {
        command.pre_exec(move || {
            if libc::fcntl(writer_fd, libc::F_SETFD, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    if let Some(identity) = pipe_identity(writer_fd) {
        command.env(PIPE_ID, identity);
    } else {
        command.env_remove(PIPE_ID);
    }
    let mut child = command.spawn()?;
    let mut streams = [
        Some(unsafe { File::from_raw_fd(child.stdout.take().unwrap().into_raw_fd()) }),
        Some(unsafe { File::from_raw_fd(child.stderr.take().unwrap().into_raw_fd()) }),
    ];
    // poll has one reader per original pipe; a failed nonblocking request
    // cannot abandon an already-running original command.
    for stream in streams.iter().flatten() {
        let _ = nonblocking(stream.as_raw_fd());
    }
    let mut stdout = std::io::stdout().lock();
    let mut stderr = std::io::stderr().lock();
    let mut private_open = true;
    let mut rows = PrivateRows::new();
    let mut stdout_count = 0usize;
    let mut forward_failed = false;
    let mut buffer = [0u8; 65536];
    while streams.iter().any(Option::is_some) {
        let mut fds = [
            libc::pollfd {
                fd: streams[0].as_ref().map_or(-1, AsRawFd::as_raw_fd),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: streams[1].as_ref().map_or(-1, AsRawFd::as_raw_fd),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: if private_open {
                    private.as_raw_fd()
                } else {
                    -1
                },
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        if unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, 50) } < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            forward_failed = true;
            break;
        }
        for index in 0..3 {
            if fds[index].revents == 0 {
                continue;
            }
            let count =
                unsafe { libc::read(fds[index].fd, buffer.as_mut_ptr().cast(), buffer.len()) };
            if count < 0 {
                if matches!(
                    std::io::Error::last_os_error().kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                ) {
                    continue;
                }
                if index < 2 {
                    streams[index] = None;
                    forward_failed = true;
                } else {
                    private_open = false;
                    rows.bad = true;
                }
            } else if count == 0 {
                if index < 2 {
                    streams[index] = None;
                } else {
                    private_open = false;
                }
            } else {
                let raw = &buffer[..count as usize];
                if index == 2 {
                    rows.push(raw);
                } else {
                    let result = if index == 0 {
                        stdout_count = stdout_count.saturating_add(raw.len());
                        stdout.write_all(raw)
                    } else {
                        stderr.write_all(raw)
                    };
                    if result.is_err() {
                        streams[index] = None;
                        forward_failed = true;
                    }
                }
            }
        }
    }
    drop(streams);
    let status = child.wait()?;
    let final_start = Instant::now();
    let mut final_bytes = 0usize;
    while private_open && final_bytes < 65536 && final_start.elapsed() < Duration::from_millis(10) {
        let count = unsafe {
            libc::read(
                private.as_raw_fd(),
                buffer.as_mut_ptr().cast(),
                (65536 - final_bytes).min(4096),
            )
        };
        if count <= 0 {
            break;
        }
        final_bytes += count as usize;
        rows.push(&buffer[..count as usize]);
    }
    if final_bytes >= 65536 || final_start.elapsed() >= Duration::from_millis(10) {
        rows.bad = true;
    }
    // Keep a live reference to the admitted WRITER through all observations.
    // Do not wait for private EOF: descendants may retain their own writer.
    drop(writer);
    drop(private);
    let _ = stdout.flush();
    let _ = stderr.flush();
    if status.success() && !forward_failed && stdout_count <= 1024 * 1024 {
        if let Some(request) = rows::request(&payload.project, &rows.records()) {
            let response = codestory_runtime::search_source_context::search_source_context(
                &request,
                &Default::default(),
            );
            let _ = stdout.write_all(&render(&response, true));
            let _ = stdout.flush();
        }
    }
    finish(status)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_start_config_attestation_is_optional_unknown_and_strictly_typed() {
        let old = serde_json::json!({"kind":"start","id":"one","real":"/rg","native":[1,2],"argv":["-n","red|blue","source.rs"],"cwd":"/project","real_pid":12});
        let parsed: Frame = serde_json::from_value(old.clone()).unwrap();
        assert!(matches!(
            &parsed,
            Frame::Start {
                config_path_absent: None,
                ..
            }
        ));
        assert_eq!(serde_json::to_value(parsed).unwrap(), old);
        let mut known = old.clone();
        known["config_path_absent"] = serde_json::json!(true);
        assert!(matches!(
            serde_json::from_value::<Frame>(known).unwrap(),
            Frame::Start {
                config_path_absent: Some(true),
                ..
            }
        ));
        let mut bad = old;
        bad["config_path_absent"] = serde_json::json!("unknown");
        assert!(serde_json::from_value::<Frame>(bad).is_err());
    }

    #[test]
    fn optional_attestation_cannot_overflow_a_previously_valid_atomic_start_frame() {
        let make = |argument: String, config_path_absent| Frame::Start {
            id: "one".into(),
            real: "/rg".into(),
            native: [1, 2],
            argv: vec![argument],
            cwd: "/project".into(),
            real_pid: 12,
            config_path_absent,
        };
        let empty = serde_json::to_vec(&make(String::new(), None))
            .unwrap()
            .len()
            + 1;
        let argument = "x".repeat(4096 - empty);
        let old = make(argument.clone(), None);
        assert_eq!(serde_json::to_vec(&old).unwrap().len() + 1, 4096);
        let known = make(argument.clone(), Some(true));
        assert!(serde_json::to_vec(&known).unwrap().len() + 1 > 4096);
        let bounded = bounded_start(known, 4096);
        assert!(matches!(
            &bounded,
            Frame::Start {
                config_path_absent: None,
                ..
            }
        ));
        assert_eq!(
            serde_json::to_value(bounded).unwrap(),
            serde_json::to_value(old).unwrap(),
            "keep the original argv and frame instead of clipping or dropping observations"
        );
        assert!(matches!(
            bounded_start(make("red|blue".into(), Some(true)), 4096),
            Frame::Start {
                config_path_absent: Some(true),
                ..
            }
        ));
    }

    #[test]
    fn replaced_private_writer_is_refused_before_descriptor_flags_change() {
        let (_original_reader, original_writer_pin) = pipe().unwrap();
        let (_other_reader, other_writer) = pipe().unwrap();
        let target = original_writer_pin.try_clone().unwrap();
        let expected = pipe_identity(original_writer_pin.as_raw_fd()).unwrap();
        assert_eq!(
            pipe_identity(target.as_raw_fd()).as_deref(),
            Some(expected.as_str())
        );
        assert!(Sender::checked(target.as_raw_fd(), &expected).is_some());
        assert_ne!(
            pipe_identity(other_writer.as_raw_fd()).as_deref(),
            Some(expected.as_str())
        );
        // The owned target keeps the same descriptor number while its native
        // pipe writer changes. Both pipes initially have identical flag kinds.
        assert_eq!(
            unsafe { libc::dup2(other_writer.as_raw_fd(), target.as_raw_fd()) },
            target.as_raw_fd()
        );
        assert!(Sender::checked(target.as_raw_fd(), &expected).is_none());
        // An eager nonblocking mutation before the identity check would alter
        // the replacement's shared open-file flags, even though it later refuses.
        let flags = unsafe { libc::fcntl(target.as_raw_fd(), libc::F_GETFL) };
        assert!(flags >= 0);
        assert_eq!(
            unsafe { libc::fcntl(target.as_raw_fd(), libc::F_SETFL, flags & !libc::O_NONBLOCK) },
            0
        );
        let before = unsafe { libc::fcntl(target.as_raw_fd(), libc::F_GETFL) };
        assert_eq!(before & libc::O_NONBLOCK, 0);
        assert!(Sender::checked(target.as_raw_fd(), &expected).is_none());
        assert_eq!(
            unsafe { libc::fcntl(target.as_raw_fd(), libc::F_GETFL) },
            before
        );
        assert_eq!(
            pipe_identity(original_writer_pin.as_raw_fd()).as_deref(),
            Some(expected.as_str())
        );
    }
}
