use fs4::fs_std::FileExt;
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::native_runtime_layout::{
    NATIVE_RUNTIME_CURRENT_FILE, NATIVE_RUNTIME_EXECUTABLE, NATIVE_RUNTIME_FILE_LIST,
    NATIVE_RUNTIME_GENERATIONS_DIR, NATIVE_RUNTIME_SEED_MARKER_PREFIX,
    NATIVE_RUNTIME_SEED_MARKER_SUFFIX, NATIVE_RUNTIME_SEEDS_DIR,
};

const STAGING_LOCK: &str = ".codestory-native-staging.lock";
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

pub(crate) fn run() -> ExitCode {
    match prepare_runtime().and_then(execute_runtime) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("codestory-cli: native runtime activation failed: {error}");
            ExitCode::FAILURE
        }
    }
}

fn prepare_runtime() -> io::Result<PathBuf> {
    let launcher = std::env::current_exe()?;
    let root = launcher.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("launcher has no parent directory: {}", launcher.display()),
        )
    })?;
    prepare_runtime_at(root)
}

fn prepare_runtime_at(root: &Path) -> io::Result<PathBuf> {
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(root.join(STAGING_LOCK))?;
    FileExt::lock_exclusive(&lock)?;

    let candidate = root.join(NATIVE_RUNTIME_EXECUTABLE);
    let seed_id = match runtime_seed_id(&candidate) {
        Ok(seed_id) => seed_id,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return pinned_current_runtime(root)?.ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    "neither a build-tree runtime candidate nor an installed generation is available",
                )
            });
        }
        Err(error) => return Err(error),
    };
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
    let seed_id = runtime_seed_id(&runtime)?;
    let runtime_sha256 = file_sha256(&runtime)?;
    if final_generation_id(&seed_id, &runtime_sha256) != generation_id {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "native generation identity does not match its executable: {}",
                generation_dir.display()
            ),
        ));
    }
    verify_complete_generation(&generation_dir, &runtime_sha256)?;
    Ok(Some(runtime))
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
        fs::copy(
            seed_dir.join(NATIVE_RUNTIME_FILE_LIST),
            temporary.join(NATIVE_RUNTIME_FILE_LIST),
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
    let seed_id = native_seed_id(generation_dir)?;
    let runtime = generation_dir.join(NATIVE_RUNTIME_EXECUTABLE);
    require_regular_file(&runtime, "native runtime executable")?;
    if runtime_seed_id(&runtime)? != seed_id {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "native generation files do not match the executable seed marker: {}",
                generation_dir.display()
            ),
        ));
    }
    if &file_sha256(&runtime)? != runtime_sha256 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "immutable native generation has unexpected executable bytes: {}",
                runtime.display()
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
    let mut input = File::open(source)?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)?;
    io::copy(&mut input, &mut output)?;
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
        NATIVE_RUNTIME_GENERATIONS_DIR, NATIVE_RUNTIME_SEEDS_DIR, final_generation_id,
        native_seed_id, prepare_runtime_at, seed_id_from_bytes,
    };
    use std::fs;

    const SEED: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

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
    }
}

#[cfg(windows)]
fn execute_runtime(path: PathBuf) -> io::Result<ExitCode> {
    let status = Command::new(path)
        .args(std::env::args_os().skip(1))
        .status()?;
    Ok(status
        .code()
        .and_then(|code| u8::try_from(code).ok())
        .map_or(ExitCode::FAILURE, ExitCode::from))
}
