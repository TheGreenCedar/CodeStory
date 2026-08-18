use codestory_contracts::bounded_locks::{
    FileLockKind, LockDeadline, PUBLICATION_LOCK_WAIT, acquire_with_deadline,
};
use sha2::{Digest, Sha256};
use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crate::native_runtime_layout::{
    NATIVE_RUNTIME_CURRENT_FILE, NATIVE_RUNTIME_EXECUTABLE, NATIVE_RUNTIME_FILE_LIST,
    NATIVE_RUNTIME_GENERATIONS_DIR, NATIVE_RUNTIME_SEED_MARKER_PREFIX,
    NATIVE_RUNTIME_SEED_MARKER_SUFFIX, NATIVE_RUNTIME_SEEDS_DIR,
};

const STAGING_LOCK: &str = ".codestory-native-staging.lock";
/// A sibling launcher that is staging a generation copies and verifies the
/// whole runtime tree, which is the same publication-class hold every other
/// long budget names. It exists so a wedged staging process can never hold a
/// new launcher for the session's lifetime.
///
/// This wait runs before the runtime exists, so it has no cancellation flag to
/// inherit and is uninterruptible for its whole budget. That is sound only
/// because no thread here is joined against a quiescence budget: the launcher
/// process has no activation worker and installs no fail-stop hook.
const STAGING_LOCK_WAIT: Duration = PUBLICATION_LOCK_WAIT;
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

pub(crate) fn run() -> ExitCode {
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    match prepare_runtime(&args).and_then(execute_runtime) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("codestory-cli: native runtime activation failed: {error}");
            ExitCode::FAILURE
        }
    }
}

fn prepare_runtime(args: &[OsString]) -> io::Result<PathBuf> {
    let launcher = std::env::current_exe()?;
    let root = launcher.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("launcher has no parent directory: {}", launcher.display()),
        )
    })?;
    prepare_runtime_for_args_at(root, args)
}

fn prepare_runtime_for_args_at(root: &Path, args: &[OsString]) -> io::Result<PathBuf> {
    if is_observational_hook_status(args) {
        return pinned_current_runtime(root)?.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "hook status requires an already-published native runtime generation",
            )
        });
    }
    prepare_runtime_at(root)
}

fn is_observational_hook_status(args: &[OsString]) -> bool {
    args.first()
        .is_some_and(|arg| arg == OsStr::new("internal-dirty-hook"))
        && args.get(1).is_some_and(|arg| arg == OsStr::new("status"))
}

fn prepare_runtime_at(root: &Path) -> io::Result<PathBuf> {
    // Installed archives ship a published generation and no build-tree candidate, so the common
    // case stages nothing. Deciding that before taking the lock keeps every ordinary invocation off
    // a machine-wide exclusive lock, and lets an install the caller cannot write to still run.
    let candidate = root.join(NATIVE_RUNTIME_EXECUTABLE);
    match fs::symlink_metadata(&candidate) {
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return pinned_current_runtime(root)?.ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    "neither a build-tree runtime candidate nor an installed generation is available",
                )
            });
        }
        Err(error) => return Err(error),
    }

    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(root.join(STAGING_LOCK))
        .map_err(|error| staging_lock_error(root, error))?;
    acquire_with_deadline(
        &lock,
        FileLockKind::Exclusive,
        LockDeadline::after(STAGING_LOCK_WAIT),
        None,
    )
    .map_err(|error| io::Error::other(error.to_string()))?;

    let seed_id = runtime_seed_id(&candidate)?;
    let seed_dir = root.join(NATIVE_RUNTIME_SEEDS_DIR).join(&seed_id);
    if !seed_dir.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("native runtime seed directory is missing for candidate {seed_id}"),
        ));
    }
    let runtime_sha256 = file_sha256(&candidate)?;
    let generation_id = final_generation_id(&seed_id, &runtime_sha256);
    let generation_dir = root
        .join(NATIVE_RUNTIME_GENERATIONS_DIR)
        .join(&generation_id);
    let runtime = generation_dir.join(NATIVE_RUNTIME_EXECUTABLE);

    fs::create_dir_all(root.join(NATIVE_RUNTIME_GENERATIONS_DIR))?;
    if generation_dir.exists() {
        verify_complete_generation(&generation_dir, &runtime_sha256)?;
    } else {
        publish_complete_generation(&seed_dir, &candidate, &generation_dir, &runtime_sha256)?;
    }
    write_atomic(
        &root.join(NATIVE_RUNTIME_CURRENT_FILE),
        format!("{generation_id}\n").as_bytes(),
    )?;
    Ok(runtime)
}

fn pinned_current_runtime(root: &Path) -> io::Result<Option<PathBuf>> {
    let pointer = root.join(NATIVE_RUNTIME_CURRENT_FILE);
    let generation_id = match fs::read_to_string(&pointer) {
        Ok(value) => value.trim().to_owned(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if generation_id.len() != 64 || !generation_id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "native generation pointer is invalid: {}",
                pointer.display()
            ),
        ));
    }
    let generation_dir = root
        .join(NATIVE_RUNTIME_GENERATIONS_DIR)
        .join(&generation_id);
    let runtime = generation_dir.join(NATIVE_RUNTIME_EXECUTABLE);
    let identity = RuntimeIdentity::read(&runtime)?;
    if final_generation_id(&identity.seed_id, &identity.sha256) != generation_id {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "native generation identity does not match its executable: {}",
                generation_dir.display()
            ),
        ));
    }
    verify_complete_generation_with(&generation_dir, &identity)?;
    Ok(Some(runtime))
}

fn staging_lock_error(root: &Path, error: io::Error) -> io::Error {
    if error.kind() != io::ErrorKind::PermissionDenied {
        return error;
    }
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        format!(
            "cannot write the native staging lock in {}. A shared or read-only install must \
             already contain a published generation; reinstall as the user that runs CodeStory, \
             or grant that user write access to this directory.",
            root.display()
        ),
    )
}

/// The runtime executable's seed marker and digest, read in one pass.
///
/// Both were previously computed by separate whole-file reads, and the pinned path did each of them
/// twice. The executable carries the embedded model in a release build, so each read is hundreds of
/// megabytes.
struct RuntimeIdentity {
    seed_id: String,
    sha256: [u8; 32],
}

impl RuntimeIdentity {
    fn read(path: &Path) -> io::Result<Self> {
        require_regular_file(path, "native runtime executable")?;
        let mut file = File::open(path)?;
        let mut hasher = Sha256::new();
        let mut buffer = vec![0_u8; 1024 * 1024];
        // The seed marker can straddle a read boundary, so each pass keeps the last
        // marker-length-minus-one bytes of the previous chunk.
        let overlap = NATIVE_RUNTIME_SEED_MARKER_PREFIX.len().saturating_sub(1);
        let mut carry: Vec<u8> = Vec::with_capacity(overlap + buffer.len());
        let mut consumed = 0_usize;
        let mut offsets = Vec::new();
        loop {
            let count = file.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            hasher.update(&buffer[..count]);
            let base = consumed.saturating_sub(carry.len());
            carry.extend_from_slice(&buffer[..count]);
            for (offset, window) in carry
                .windows(NATIVE_RUNTIME_SEED_MARKER_PREFIX.len())
                .enumerate()
            {
                if window == NATIVE_RUNTIME_SEED_MARKER_PREFIX {
                    offsets.push(base + offset);
                    // More than one marker is already a failure; stop growing the list.
                    if offsets.len() > 1 {
                        break;
                    }
                }
            }
            consumed += count;
            let keep = carry.len().min(overlap);
            carry.drain(..carry.len() - keep);
        }
        if offsets.len() != 1 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "runtime executable has no unique native seed marker: {}",
                    path.display()
                ),
            ));
        }
        let seed_id = read_seed_id_at(path, offsets[0])?;
        Ok(Self {
            seed_id,
            sha256: hasher.finalize().into(),
        })
    }
}

fn final_generation_id(seed_id: &str, runtime_sha256: &[u8; 32]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"codestory-native-executable-generation-v1\0");
    hasher.update(seed_id.as_bytes());
    hasher.update([0]);
    hasher.update(runtime_sha256);
    format!("{:x}", hasher.finalize())
}

fn publish_complete_generation(
    seed_dir: &Path,
    candidate: &Path,
    generation_dir: &Path,
    runtime_sha256: &[u8; 32],
) -> io::Result<()> {
    let names = runtime_file_names(seed_dir)?;
    let parent = generation_dir
        .parent()
        .expect("generation directory has a parent");
    let temporary = parent.join(format!(
        ".{}.codestory-stage-{}-{}.tmp",
        generation_dir
            .file_name()
            .expect("generation has a file name")
            .to_string_lossy(),
        std::process::id(),
        TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&temporary)?;
    let result = (|| {
        for name in &names {
            hard_link_or_copy(&seed_dir.join(name), &temporary.join(name))?;
        }
        copy_verified(candidate, &temporary.join(NATIVE_RUNTIME_EXECUTABLE))?;
        copy_verified(
            &seed_dir.join(NATIVE_RUNTIME_FILE_LIST),
            &temporary.join(NATIVE_RUNTIME_FILE_LIST),
        )?;
        sync_directory(&temporary)?;
        fs::rename(&temporary, generation_dir)?;
        sync_parent(generation_dir)?;
        verify_complete_generation(generation_dir, runtime_sha256)
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&temporary);
    }
    result
}

fn verify_complete_generation(generation_dir: &Path, runtime_sha256: &[u8; 32]) -> io::Result<()> {
    let runtime = generation_dir.join(NATIVE_RUNTIME_EXECUTABLE);
    let identity = RuntimeIdentity::read(&runtime)?;
    if &identity.sha256 != runtime_sha256 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "immutable native generation has unexpected executable bytes: {}",
                runtime.display()
            ),
        ));
    }
    verify_complete_generation_with(generation_dir, &identity)
}

/// Check a generation against an executable identity the caller already computed.
///
/// The executable is the expensive artifact to read, so it is never re-read here.
fn verify_complete_generation_with(
    generation_dir: &Path,
    identity: &RuntimeIdentity,
) -> io::Result<()> {
    let seed_id = native_seed_id(generation_dir)?;
    if identity.seed_id != seed_id {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "native generation files do not match the executable seed marker: {}",
                generation_dir.display()
            ),
        ));
    }
    Ok(())
}

fn native_seed_id(directory: &Path) -> io::Result<String> {
    let names = runtime_file_names(directory)?;
    let mut hasher = Sha256::new();
    hasher.update(b"codestory-native-generation-v1\0");
    for name in names {
        let artifact = directory.join(&name);
        require_regular_file(&artifact, "native runtime artifact")?;
        hasher.update(name.as_bytes());
        hasher.update([0]);
        hasher.update(file_sha256(&artifact)?);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn runtime_file_names(directory: &Path) -> io::Result<Vec<String>> {
    let manifest = directory.join(NATIVE_RUNTIME_FILE_LIST);
    let names = fs::read_to_string(&manifest)?
        .lines()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if names.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("native runtime manifest is empty: {}", manifest.display()),
        ));
    }
    let mut sorted = names.clone();
    sorted.sort_by_key(|name| name.to_lowercase());
    sorted.dedup_by(|left, right| left.eq_ignore_ascii_case(right));
    if names != sorted || names.iter().any(|name| !safe_file_name(name)) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("native runtime manifest is invalid: {}", manifest.display()),
        ));
    }
    Ok(names)
}

fn safe_file_name(name: &str) -> bool {
    !name.is_empty()
        && !matches!(name, "." | "..")
        && !name.contains(['/', '\\'])
        && Path::new(name).file_name().and_then(|value| value.to_str()) == Some(name)
}

fn runtime_seed_id(path: &Path) -> io::Result<String> {
    require_regular_file(path, "native runtime executable")?;
    let bytes = fs::read(path)?;
    seed_id_from_bytes(&bytes).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "runtime executable has no unique native seed marker: {}",
                path.display()
            ),
        )
    })
}

/// Read and validate the seed id that follows a marker at `offset`.
fn read_seed_id_at(path: &Path, offset: usize) -> io::Result<String> {
    use std::io::{Seek, SeekFrom};

    let invalid = || {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "runtime executable has no unique native seed marker: {}",
                path.display()
            ),
        )
    };
    let id_start = offset + NATIVE_RUNTIME_SEED_MARKER_PREFIX.len();
    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(
        u64::try_from(id_start).map_err(|_| invalid())?,
    ))?;
    let mut framed = vec![0_u8; 64 + NATIVE_RUNTIME_SEED_MARKER_SUFFIX.len()];
    file.read_exact(&mut framed).map_err(|_| invalid())?;
    if &framed[64..] != NATIVE_RUNTIME_SEED_MARKER_SUFFIX {
        return Err(invalid());
    }
    std::str::from_utf8(&framed[..64])
        .ok()
        .filter(|value| value.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .map(str::to_owned)
        .ok_or_else(invalid)
}

fn seed_id_from_bytes(bytes: &[u8]) -> Option<String> {
    let matches = bytes
        .windows(NATIVE_RUNTIME_SEED_MARKER_PREFIX.len())
        .enumerate()
        .filter_map(|(offset, window)| {
            (window == NATIVE_RUNTIME_SEED_MARKER_PREFIX).then_some(offset)
        })
        .collect::<Vec<_>>();
    if matches.len() == 1 {
        let id_start = matches[0] + NATIVE_RUNTIME_SEED_MARKER_PREFIX.len();
        let id_end = id_start + 64;
        if bytes.get(id_end..id_end + NATIVE_RUNTIME_SEED_MARKER_SUFFIX.len())
            == Some(NATIVE_RUNTIME_SEED_MARKER_SUFFIX)
        {
            let id = bytes
                .get(id_start..id_end)
                .and_then(|value| std::str::from_utf8(value).ok())
                .filter(|value| value.bytes().all(|byte| byte.is_ascii_hexdigit()));
            if let Some(id) = id {
                return Some(id.to_owned());
            }
        }
    }
    None
}

fn require_regular_file(path: &Path, label: &str) -> io::Result<()> {
    if fs::metadata(path)?.is_file() && !fs::symlink_metadata(path)?.file_type().is_symlink() {
        return Ok(());
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        format!("{label} is not a regular file: {}", path.display()),
    ))
}

fn hard_link_or_copy(source: &Path, destination: &Path) -> io::Result<()> {
    require_regular_file(source, "native seed artifact")?;
    match fs::hard_link(source, destination) {
        Ok(()) => Ok(()),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::CrossesDevices
                    | io::ErrorKind::PermissionDenied
                    | io::ErrorKind::Unsupported
            ) =>
        {
            copy_verified(source, destination)
        }
        Err(error) => Err(error),
    }
}

fn copy_verified(source: &Path, destination: &Path) -> io::Result<()> {
    let expected = file_sha256(source)?;
    let permissions = fs::metadata(source)?.permissions();
    let mut input = File::open(source)?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)?;
    io::copy(&mut input, &mut output)?;
    fs::set_permissions(destination, permissions)?;
    output.sync_all()?;
    drop(output);
    if file_sha256(destination)? != expected {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("copied runtime differs from source: {}", source.display()),
        ));
    }
    Ok(())
}

fn file_sha256(path: &Path) -> io::Result<[u8; 32]> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(hasher.finalize().into())
}

fn write_atomic(destination: &Path, contents: &[u8]) -> io::Result<()> {
    let temporary = destination.with_file_name(format!(
        ".{}.codestory-stage-{}-{}.tmp",
        destination
            .file_name()
            .expect("pointer has a file name")
            .to_string_lossy(),
        std::process::id(),
        TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    let result = (|| {
        output.write_all(contents)?;
        output.sync_all()?;
        drop(output);
        replace_file(&temporary, destination)?;
        sync_parent(destination)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

#[cfg(not(windows))]
fn replace_file(source: &Path, destination: &Path) -> io::Result<()> {
    fs::rename(source, destination)
}

#[cfg(windows)]
fn replace_file(source: &Path, destination: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };

    let source = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    // SAFETY: both path buffers are null-terminated and remain live for the call.
    let result = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(windows))]
fn sync_directory(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()
}

#[cfg(windows)]
fn sync_directory(_path: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(not(windows))]
fn sync_parent(path: &Path) -> io::Result<()> {
    path.parent()
        .map_or(Ok(()), |parent| File::open(parent)?.sync_all())
}

#[cfg(windows)]
fn sync_parent(_path: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn execute_runtime(path: PathBuf) -> io::Result<ExitCode> {
    use std::os::unix::process::CommandExt;
    let error = Command::new(path).args(std::env::args_os().skip(1)).exec();
    Err(error)
}

#[cfg(test)]
mod tests {
    use super::{
        NATIVE_RUNTIME_CURRENT_FILE, NATIVE_RUNTIME_EXECUTABLE, NATIVE_RUNTIME_FILE_LIST,
        NATIVE_RUNTIME_GENERATIONS_DIR, NATIVE_RUNTIME_SEEDS_DIR, STAGING_LOCK,
        final_generation_id, job_step_error, native_seed_id, prepare_runtime_at,
        prepare_runtime_for_args_at, raw_exit_evidence, seed_id_from_bytes,
    };
    use std::{ffi::OsString, fs};

    const SEED: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    /// Build a root that has already published one generation, as an install would.
    pub(super) fn install_fixture(root: &std::path::Path) {
        let staging = root.join("seed-staging");
        fs::create_dir(&staging).expect("seed staging directory");
        fs::write(staging.join("libggml.so"), b"ggml").expect("ggml runtime");
        fs::write(staging.join("libllama.so"), b"llama").expect("llama runtime");
        fs::write(
            staging.join(NATIVE_RUNTIME_FILE_LIST),
            "libggml.so\nlibllama.so\n",
        )
        .expect("runtime manifest");
        let seed_id = native_seed_id(&staging).expect("seed identity");
        let seed = root.join(NATIVE_RUNTIME_SEEDS_DIR).join(&seed_id);
        fs::create_dir_all(seed.parent().expect("seed parent")).expect("seed root");
        fs::rename(staging, &seed).expect("publish seed");
        let candidate = root.join(NATIVE_RUNTIME_EXECUTABLE);
        fs::write(
            &candidate,
            format!("codestory-native-runtime-seed-v1|id={seed_id}|end-installed"),
        )
        .expect("runtime candidate");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = fs::metadata(&candidate)
                .expect("runtime candidate metadata")
                .permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&candidate, permissions).expect("executable runtime candidate");
        }
        prepare_runtime_at(root).expect("publish the installed generation");
    }

    #[test]
    fn parses_one_exact_native_seed_marker() {
        let bytes = format!("prefix codestory-native-runtime-seed-v1|id={SEED}|end suffix");
        assert_eq!(seed_id_from_bytes(bytes.as_bytes()).as_deref(), Some(SEED));
    }

    #[test]
    fn rejects_ambiguous_or_malformed_native_seed_markers() {
        let marker = format!("codestory-native-runtime-seed-v1|id={SEED}|end");
        assert!(seed_id_from_bytes(format!("{marker}{marker}").as_bytes()).is_none());
        assert!(
            seed_id_from_bytes(b"codestory-native-runtime-seed-v1|id=not-a-sha256|end").is_none()
        );
    }

    #[test]
    fn records_raw_exit_codes_that_do_not_fit_the_launcher_status() {
        let evidence =
            raw_exit_evidence(Some(-1073741819)).expect("ntstatus-shaped code leaves evidence");
        assert!(evidence.contains("-1073741819"), "raw value survives");
        assert!(
            evidence.contains("0xc0000005"),
            "ntstatus form accompanies it"
        );
        assert!(raw_exit_evidence(Some(256)).is_some());
    }

    #[test]
    fn forwardable_exit_codes_leave_no_raw_evidence() {
        assert_eq!(raw_exit_evidence(Some(0)), None);
        assert_eq!(raw_exit_evidence(Some(255)), None);
        assert_eq!(raw_exit_evidence(None), None);
    }

    #[test]
    fn job_object_failures_name_the_step_that_failed() {
        let denied = std::io::Error::from_raw_os_error(5);
        let kind = denied.kind();
        let error = job_step_error("assign runtime child to its job object", denied);
        assert!(
            error
                .to_string()
                .starts_with("assign runtime child to its job object: "),
            "the failing step survives into the rendered failure"
        );
        assert_eq!(error.kind(), kind, "the classification is preserved");
    }

    #[test]
    fn hook_status_does_not_stage_a_build_tree_runtime() {
        let temp = tempfile::tempdir().expect("temporary runtime root");
        let root = temp.path();
        fs::write(root.join(NATIVE_RUNTIME_EXECUTABLE), b"build-tree launcher")
            .expect("build-tree candidate");
        let args = [
            OsString::from("internal-dirty-hook"),
            OsString::from("status"),
        ];

        let error = prepare_runtime_for_args_at(root, &args)
            .expect_err("status must not stage an unpublished build-tree candidate");

        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
        assert!(!root.join(STAGING_LOCK).exists(), "no staging lock created");
        assert!(
            !root.join(NATIVE_RUNTIME_GENERATIONS_DIR).exists(),
            "no generation directory created"
        );
        assert!(
            !root.join(NATIVE_RUNTIME_CURRENT_FILE).exists(),
            "no current-generation pointer created"
        );
    }

    #[test]
    fn executable_bytes_participate_in_the_generation_identity() {
        assert_ne!(
            final_generation_id(SEED, &[1; 32]),
            final_generation_id(SEED, &[2; 32])
        );
    }

    #[test]
    fn publishes_and_pins_one_complete_executable_generation() {
        let temp = tempfile::tempdir().expect("temporary runtime root");
        let root = temp.path();
        let staging = root.join("seed-staging");
        fs::create_dir(&staging).expect("seed staging directory");
        fs::write(staging.join("libggml.so"), b"ggml").expect("ggml runtime");
        fs::write(staging.join("libllama.so"), b"llama").expect("llama runtime");
        fs::write(
            staging.join(NATIVE_RUNTIME_FILE_LIST),
            "libggml.so\nlibllama.so\n",
        )
        .expect("runtime manifest");
        let seed_id = native_seed_id(&staging).expect("seed identity");
        let seed = root.join(NATIVE_RUNTIME_SEEDS_DIR).join(&seed_id);
        fs::create_dir_all(seed.parent().expect("seed parent")).expect("seed root");
        fs::rename(staging, &seed).expect("publish seed");
        let candidate = root.join(NATIVE_RUNTIME_EXECUTABLE);
        fs::write(
            &candidate,
            format!("codestory-native-runtime-seed-v1|id={seed_id}|end-v1"),
        )
        .expect("runtime candidate");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = fs::metadata(&candidate)
                .expect("runtime candidate metadata")
                .permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&candidate, permissions).expect("executable runtime candidate");
        }

        let first = prepare_runtime_at(root).expect("first activation");
        assert_eq!(
            fs::read(&first).expect("pinned runtime"),
            fs::read(&candidate).expect("candidate runtime")
        );
        let first_generation = fs::read_to_string(root.join(NATIVE_RUNTIME_CURRENT_FILE))
            .expect("current generation")
            .trim()
            .to_owned();
        assert!(first.starts_with(root.join(NATIVE_RUNTIME_GENERATIONS_DIR)));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_ne!(
                fs::metadata(&first)
                    .expect("published runtime metadata")
                    .permissions()
                    .mode()
                    & 0o111,
                0,
                "published runtime remains executable"
            );
        }

        fs::write(
            &candidate,
            format!("codestory-native-runtime-seed-v1|id={seed_id}|end-v2"),
        )
        .expect("updated runtime candidate");
        let second = prepare_runtime_at(root).expect("second activation");
        let second_generation = fs::read_to_string(root.join(NATIVE_RUNTIME_CURRENT_FILE))
            .expect("updated current generation")
            .trim()
            .to_owned();

        assert_ne!(first_generation, second_generation);
        assert_ne!(first, second);
        assert!(
            first.is_file(),
            "previous immutable generation remains pinned"
        );

        fs::remove_file(&candidate).expect("remove build-tree candidate");
        assert_eq!(
            prepare_runtime_at(root).expect("installed activation"),
            second
        );

        fs::write(&candidate, b"malformed runtime candidate").expect("malformed candidate");
        assert_eq!(
            prepare_runtime_at(root)
                .expect_err("malformed candidate must not fall back")
                .kind(),
            std::io::ErrorKind::InvalidData
        );

        let missing_seed = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";
        fs::write(
            &candidate,
            format!("codestory-native-runtime-seed-v1|id={missing_seed}|end"),
        )
        .expect("candidate with missing seed");
        assert_eq!(
            prepare_runtime_at(root)
                .expect_err("missing seed must not fall back")
                .kind(),
            std::io::ErrorKind::NotFound
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;

            fs::remove_file(&candidate).expect("remove missing-seed candidate");
            symlink(root.join("missing-runtime"), &candidate).expect("dangling runtime candidate");
            assert!(
                prepare_runtime_at(root).is_err(),
                "dangling candidate must not fall back"
            );
        }
    }
}

#[cfg(any(windows, test))]
fn raw_exit_evidence(code: Option<i32>) -> Option<String> {
    let code = code?;
    // Codes outside the launcher's 8-bit exit status — NTSTATUS crash values
    // arrive as negatives — would otherwise vanish behind the generic failure
    // classification, so the raw value is recorded before that collapse.
    u8::try_from(code).is_err().then(|| {
        format!(
            "native runtime exited with raw code {code} (0x{:08x})",
            code as u32
        )
    })
}

#[cfg(any(windows, test))]
fn job_step_error(step: &str, error: io::Error) -> io::Error {
    // Every job-object failure surfaces through the launcher's single
    // activation message, so the failing step has to be named here for a
    // field report to distinguish creation, configuration, and assignment
    // problems.
    io::Error::new(error.kind(), format!("{step}: {error}"))
}

#[cfg(windows)]
fn execute_runtime(path: PathBuf) -> io::Result<ExitCode> {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_BREAKAWAY_OK,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JobObjectExtendedLimitInformation, SetInformationJobObject,
    };

    // Windows has no process-group teardown on parent death, so the runtime
    // child is tethered to a kill-on-close job: when this launcher exits or is
    // killed, its job handle closes and the kernel reaps the child tree.
    let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
    if job.is_null() {
        return Err(job_step_error(
            "create runtime job object",
            io::Error::last_os_error(),
        ));
    }
    // SAFETY: the job handle is non-null, live, and owned by nothing else.
    let job = unsafe { OwnedHandle::from_raw_handle(job.cast()) };
    // SAFETY: all-zero extended limits are the documented "no limits" state;
    // only the two flags below are raised on top of it.
    let mut limits = unsafe { std::mem::zeroed::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() };
    // Kill-on-close scopes the runtime tree to this launcher invocation.
    // Breakaway stays permitted because the per-user embedding server outlives
    // any single invocation by design — its idle timeout owns its lifetime and
    // later commands reconnect to it instead of reloading the model — so its
    // spawn opts out of the job explicitly. Nothing else the runtime starts
    // asks to break away.
    limits.BasicLimitInformation.LimitFlags =
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_BREAKAWAY_OK;
    // SAFETY: limits is a live extended-limit block passed with its exact size
    // for this information class.
    let configured = unsafe {
        SetInformationJobObject(
            job.as_raw_handle().cast(),
            JobObjectExtendedLimitInformation,
            (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
    };
    if configured == 0 {
        return Err(job_step_error(
            "configure runtime job object limits",
            io::Error::last_os_error(),
        ));
    }

    let mut child = Command::new(path)
        .args(std::env::args_os().skip(1))
        .spawn()?;
    // Command cannot expose a suspended primary thread to resume later, so the
    // job assignment lands immediately after spawn instead — the same
    // adopt-right-after-acquisition pattern the embedding transport uses for
    // peer process handles. The unassigned window is a few launcher
    // instructions wide and only reproduces the old orphaning if the launcher
    // dies inside it.
    // SAFETY: both handles are live; the child has not been waited on yet.
    let assigned = unsafe {
        AssignProcessToJobObject(job.as_raw_handle().cast(), child.as_raw_handle().cast())
    };
    if assigned == 0 {
        let error = job_step_error(
            "assign runtime child to its job object",
            io::Error::last_os_error(),
        );
        // A child left outside the job would outlive this failed activation
        // unmanaged, which is exactly what the job exists to prevent.
        let _ = child.kill();
        let _ = child.wait();
        return Err(error);
    }
    let status = child.wait()?;
    if let Some(evidence) = raw_exit_evidence(status.code()) {
        eprintln!("codestory-cli: {evidence}");
    }
    Ok(status
        .code()
        .and_then(|code| u8::try_from(code).ok())
        .map_or(ExitCode::FAILURE, ExitCode::from))
}

#[cfg(test)]
mod installed_layout_tests {
    use super::tests::install_fixture;
    use super::{
        NATIVE_RUNTIME_EXECUTABLE, STAGING_LOCK, prepare_runtime_at, prepare_runtime_for_args_at,
    };
    use std::{ffi::OsString, fs};

    #[test]
    fn an_installed_generation_activates_without_taking_the_staging_lock() {
        // Installed archives carry a published generation and no build-tree candidate, so nothing
        // is staged. Taking a machine-wide exclusive lock anyway serialized every invocation and
        // made a read-only or shared install unusable for anyone who could not write to it.
        let temp = tempfile::tempdir().expect("temporary runtime root");
        let root = temp.path();
        install_fixture(root);
        fs::remove_file(root.join(NATIVE_RUNTIME_EXECUTABLE)).expect("remove build-tree candidate");
        fs::remove_file(root.join(STAGING_LOCK)).expect("clear the lock left by staging");

        let runtime = prepare_runtime_at(root).expect("installed activation");
        assert!(runtime.is_file(), "installed generation resolves");
        assert!(
            !root.join(STAGING_LOCK).exists(),
            "an installed activation must not create the staging lock"
        );
    }

    #[test]
    fn hook_status_uses_the_published_generation_without_staging_the_candidate() {
        let temp = tempfile::tempdir().expect("temporary runtime root");
        let root = temp.path();
        install_fixture(root);
        fs::remove_file(root.join(STAGING_LOCK)).expect("clear the lock left by staging");
        let args = [
            OsString::from("internal-dirty-hook"),
            OsString::from("status"),
        ];

        let runtime = prepare_runtime_for_args_at(root, &args).expect("published hook runtime");

        assert!(runtime.is_file());
        assert!(
            root.join(NATIVE_RUNTIME_EXECUTABLE).is_file(),
            "the build-tree candidate remains available but unused"
        );
        assert!(
            !root.join(STAGING_LOCK).exists(),
            "observational status never creates the staging lock"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_read_only_install_still_activates() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().expect("temporary runtime root");
        let root = temp.path();
        install_fixture(root);
        fs::remove_file(root.join(NATIVE_RUNTIME_EXECUTABLE)).expect("remove build-tree candidate");
        fs::remove_file(root.join(STAGING_LOCK)).expect("clear the lock left by staging");

        let original = fs::metadata(root).expect("root metadata").permissions();
        let mut read_only = original.clone();
        read_only.set_mode(0o555);
        fs::set_permissions(root, read_only).expect("make the install read-only");

        let activation = prepare_runtime_at(root);

        fs::set_permissions(root, original).expect("restore permissions for cleanup");
        assert!(
            activation.expect("read-only install activates").is_file(),
            "a shared or read-only install must still run"
        );
    }
}
