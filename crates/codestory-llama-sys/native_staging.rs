use fs4::fs_std::FileExt;
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;
#[cfg(windows)]
use std::time::Duration;
#[cfg(windows)]
use windows_sys::Win32::Storage::FileSystem::ReplaceFileW;

const UPSTREAM_BUILD_SUPPORT_LIBRARY_NAMES: &[&str] = &["libllama-common.so"];
static STAGING_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Copy one Windows runtime DLL without exposing a missing or partial destination.
#[cfg(test)]
pub(crate) fn stage_windows_runtime_file(source: &Path, destination: &Path) -> io::Result<()> {
    stage_windows_runtime_files(&[(source, destination)])
}

/// Prepare every Windows runtime DLL before publishing the transaction.
///
/// The upstream build can leave hard links in Cargo's runtime directory.
/// Copying to unique same-directory files breaks those links. The publication
/// transaction snapshots every destination before its first replacement and
/// restores the complete previous set if a later replacement fails.
pub(crate) fn stage_windows_runtime_files(entries: &[(&Path, &Path)]) -> io::Result<()> {
    let Some(profile_dir) = entries
        .first()
        .and_then(|(_, destination)| destination.parent())
    else {
        return Ok(());
    };
    if entries
        .iter()
        .any(|(_, destination)| destination.parent() != Some(profile_dir))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Windows runtime destinations must share one profile directory",
        ));
    }
    let _lock = acquire_profile_staging_lock(profile_dir)?;
    let mut replacements = Vec::with_capacity(entries.len());
    for (source, destination) in entries {
        replacements.push(prepare_runtime_copy(source, destination)?);
    }
    publish_all(replacements)
}

/// Stage real Linux shared-library files into every Cargo runtime directory.
///
/// `llama-cpp-sys-2` hard-links only the unversioned `.so` symlinks into these
/// directories. The links are therefore dangling, so its next build observes
/// `Path::exists() == false` and then fails to create the already-present link.
/// It also omits dynamically loaded backend modules from `deps` and `examples`,
/// even though test and example executables resolve runtime modules beside the
/// executable. Replacing the already validated runtime files with hard links to
/// their resolved regular files fixes both boundaries without duplicating bytes
/// or staging libraries outside the runtime manifest. The pinned upstream build
/// also places a dangling `libllama-common.so` link in these directories. That
/// build-support library remains outside CodeStory's package manifest; this
/// helper refreshes its entry where upstream already created one so a reused
/// target directory cannot retain bytes from an older dependency build.
pub(crate) fn stage_linux_shared_libraries(
    runtime_sources: &[&Path],
    upstream_build_support_sources: &[&Path],
    profile_dir: &Path,
) -> io::Result<()> {
    let _lock = acquire_profile_staging_lock(profile_dir)?;
    let mut sources = runtime_sources.to_vec();
    for source in &sources {
        if !source.is_file() || !is_linux_shared_library_name(source) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "invalid Linux runtime shared-library source: {}",
                    source.display()
                ),
            ));
        }
    }
    for source in upstream_build_support_sources {
        let Some(name) = source.file_name().and_then(|name| name.to_str()) else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "invalid upstream build-support source: {}",
                    source.display()
                ),
            ));
        };
        if !source.is_file() || !UPSTREAM_BUILD_SUPPORT_LIBRARY_NAMES.contains(&name) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "unexpected upstream build-support source: {}",
                    source.display()
                ),
            ));
        }
    }
    sources.sort_by(|left, right| left.file_name().cmp(&right.file_name()));
    for pair in sources.windows(2) {
        if pair[0].file_name() == pair[1].file_name() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!(
                    "duplicate Linux shared-library source: {}",
                    pair[0]
                        .file_name()
                        .expect("filtered source has a file name")
                        .to_string_lossy()
                ),
            ));
        }
    }

    let destinations = [
        profile_dir.to_path_buf(),
        profile_dir.join("deps"),
        profile_dir.join("examples"),
    ];
    let mut replacements = Vec::new();
    for destination_dir in destinations {
        fs::create_dir_all(&destination_dir)?;
        for source in &sources {
            let name = source
                .file_name()
                .expect("filtered shared-library source has a file name");
            replacements.push(prepare_real_hard_link(source, &destination_dir.join(name))?);
        }
        for source in upstream_build_support_sources {
            if let Some(replacement) =
                prepare_preexisting_build_support_link(source, &destination_dir)?
            {
                replacements.push(replacement);
            }
        }
    }
    publish_all(replacements)
}

fn is_linux_shared_library_name(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    name.starts_with("lib") && (name.ends_with(".so") || name.contains(".so."))
}

fn prepare_preexisting_build_support_link(
    source: &Path,
    destination_dir: &Path,
) -> io::Result<Option<PreparedReplacement>> {
    let destination = destination_dir.join(
        source
            .file_name()
            .expect("validated build-support source has a file name"),
    );
    match fs::symlink_metadata(&destination) {
        Ok(_) => prepare_real_hard_link(source, &destination).map(Some),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn prepare_real_hard_link(source: &Path, destination: &Path) -> io::Result<PreparedReplacement> {
    let source = fs::canonicalize(source)?;
    validate_regular_source(&source, "Linux shared-library")?;
    validate_destination(destination, "shared-library", true)?;
    let temporary = create_unique_hard_link(&source, destination)?;
    Ok(PreparedReplacement::new(temporary, destination))
}

fn prepare_runtime_copy(source: &Path, destination: &Path) -> io::Result<PreparedReplacement> {
    validate_regular_source(source, "Windows runtime")?;
    validate_destination(destination, "native runtime", false)?;

    let (temporary, mut output) = create_unique_temp_file(destination)?;
    let result = (|| {
        let mut input = File::open(source)?;
        io::copy(&mut input, &mut output)?;
        output.sync_all()?;
        drop(output);
        if file_sha256(source)? != file_sha256(&temporary)? {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "staged native runtime copy differs from source: {}",
                    source.display()
                ),
            ));
        }
        Ok(())
    })();
    if let Err(error) = result {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    Ok(PreparedReplacement::new(temporary, destination))
}

fn validate_regular_source(source: &Path, label: &str) -> io::Result<()> {
    let metadata = fs::metadata(source)?;
    if metadata.is_file() {
        return Ok(());
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidInput,
        format!("{label} source is not a regular file: {}", source.display()),
    ))
}

fn validate_destination(destination: &Path, label: &str, allow_symlink: bool) -> io::Result<()> {
    match fs::symlink_metadata(destination) {
        Ok(metadata) if metadata.file_type().is_dir() => Err(io::Error::new(
            io::ErrorKind::IsADirectory,
            format!(
                "{label} destination is a directory: {}",
                destination.display()
            ),
        )),
        Ok(metadata) if metadata.file_type().is_symlink() && !allow_symlink => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "{label} destination is a symlink: {}",
                destination.display()
            ),
        )),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn create_unique_temp_file(destination: &Path) -> io::Result<(PathBuf, File)> {
    loop {
        let temporary = staging_temp_path(destination);
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(file) => return Ok((temporary, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
}

fn create_unique_hard_link(source: &Path, destination: &Path) -> io::Result<PathBuf> {
    loop {
        let temporary = staging_temp_path(destination);
        match fs::hard_link(source, &temporary) {
            Ok(()) => return Ok(temporary),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
}

fn staging_temp_path(destination: &Path) -> PathBuf {
    let counter = STAGING_COUNTER.fetch_add(1, Ordering::Relaxed);
    let name = destination
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_else(|| "native-runtime".into());
    destination.with_file_name(format!(
        ".{name}.codestory-stage-{}-{counter}.tmp",
        std::process::id()
    ))
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

fn acquire_profile_staging_lock(profile_dir: &Path) -> io::Result<File> {
    fs::create_dir_all(profile_dir)?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(profile_dir.join(".codestory-native-staging.lock"))?;
    FileExt::lock_exclusive(&lock)?;
    Ok(lock)
}

fn publish_all(replacements: Vec<PreparedReplacement>) -> io::Result<()> {
    publish_all_with(replacements, replace_file)
}

fn publish_all_with<F>(
    mut replacements: Vec<PreparedReplacement>,
    mut publish_replacement: F,
) -> io::Result<()>
where
    F: FnMut(&Path, &Path) -> io::Result<()>,
{
    let mut destinations = std::collections::HashSet::with_capacity(replacements.len());
    for replacement in &replacements {
        if !destinations.insert(destination_key(&replacement.destination)) {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!(
                    "duplicate native runtime destination: {}",
                    replacement.destination.display()
                ),
            ));
        }
    }

    let mut transaction = PublicationTransaction::capture(&replacements)?;
    for (index, replacement) in replacements.iter_mut().enumerate() {
        if let Err(error) = replacement.publish_with(&mut publish_replacement) {
            return Err(transaction.rollback(index, error));
        }
    }
    if let Err(error) = sync_destination_directories(&replacements) {
        return Err(transaction.rollback(replacements.len(), error));
    }
    Ok(())
}

fn sync_destination_directories(replacements: &[PreparedReplacement]) -> io::Result<()> {
    let mut directories = std::collections::HashSet::new();
    for replacement in replacements {
        if let Some(parent) = replacement.destination.parent() {
            let key = destination_key(parent);
            if directories.insert(key) {
                sync_parent_directory(&replacement.destination)?;
            }
        }
    }
    Ok(())
}

fn destination_key(path: &Path) -> String {
    let path = path.to_string_lossy();
    if cfg!(windows) {
        path.to_lowercase()
    } else {
        path.into_owned()
    }
}

struct PreparedReplacement {
    temporary: Option<PathBuf>,
    destination: PathBuf,
}

impl PreparedReplacement {
    fn new(temporary: PathBuf, destination: &Path) -> Self {
        Self {
            temporary: Some(temporary),
            destination: destination.to_path_buf(),
        }
    }

    fn publish_with<F>(&mut self, publish_replacement: &mut F) -> io::Result<()>
    where
        F: FnMut(&Path, &Path) -> io::Result<()>,
    {
        let temporary = self
            .temporary
            .as_ref()
            .expect("prepared replacement has a temporary file");
        publish_replacement(temporary, &self.destination)?;
        self.temporary = None;
        Ok(())
    }
}

impl Drop for PreparedReplacement {
    fn drop(&mut self) {
        if let Some(temporary) = &self.temporary {
            let _ = fs::remove_file(temporary);
        }
    }
}

struct PublicationTransaction {
    snapshots: Vec<DestinationSnapshot>,
}

impl PublicationTransaction {
    fn capture(replacements: &[PreparedReplacement]) -> io::Result<Self> {
        let snapshots = replacements
            .iter()
            .map(|replacement| DestinationSnapshot::capture(&replacement.destination))
            .collect::<io::Result<Vec<_>>>()?;
        Ok(Self { snapshots })
    }

    fn rollback(&mut self, published_count: usize, publish_error: io::Error) -> io::Error {
        let mut rollback_errors = Vec::new();
        for snapshot in self.snapshots[..published_count].iter_mut().rev() {
            if let Err(error) = snapshot.restore() {
                rollback_errors.push(format!("{}: {error}", snapshot.destination.display()));
            }
        }
        if let Err(error) = sync_snapshot_directories(&self.snapshots[..published_count]) {
            rollback_errors.push(format!("sync restored directories: {error}"));
        }
        if rollback_errors.is_empty() {
            return publish_error;
        }
        io::Error::other(format!(
            "{publish_error}; native runtime rollback also failed: {}",
            rollback_errors.join("; ")
        ))
    }
}

struct DestinationSnapshot {
    destination: PathBuf,
    previous: PreviousDestination,
}

impl DestinationSnapshot {
    fn capture(destination: &Path) -> io::Result<Self> {
        let previous = match fs::symlink_metadata(destination) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                PreviousDestination::Symlink(fs::read_link(destination)?)
            }
            Ok(metadata) if metadata.is_file() => {
                PreviousDestination::File(create_unique_hard_link(destination, destination)?)
            }
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "native runtime destination cannot be snapshotted: {}",
                        destination.display()
                    ),
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => PreviousDestination::Absent,
            Err(error) => return Err(error),
        };
        Ok(Self {
            destination: destination.to_path_buf(),
            previous,
        })
    }

    fn restore(&mut self) -> io::Result<()> {
        match &mut self.previous {
            PreviousDestination::Absent => match fs::remove_file(&self.destination) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error),
            },
            PreviousDestination::File(temporary) => {
                replace_file(temporary, &self.destination)?;
                self.previous = PreviousDestination::Absent;
                Ok(())
            }
            #[cfg(unix)]
            PreviousDestination::Symlink(target) => {
                let temporary = create_unique_symlink(target, &self.destination)?;
                let result = replace_file(&temporary, &self.destination);
                if result.is_err() {
                    let _ = fs::remove_file(&temporary);
                }
                result
            }
            #[cfg(not(unix))]
            PreviousDestination::Symlink(_) => Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "native runtime symlink rollback is unsupported on this platform",
            )),
        }
    }
}

impl Drop for DestinationSnapshot {
    fn drop(&mut self) {
        if let PreviousDestination::File(temporary) = &self.previous {
            let _ = fs::remove_file(temporary);
        }
    }
}

enum PreviousDestination {
    Absent,
    File(PathBuf),
    Symlink(PathBuf),
}

#[cfg(unix)]
fn create_unique_symlink(target: &Path, destination: &Path) -> io::Result<PathBuf> {
    use std::os::unix::fs::symlink;

    loop {
        let temporary = staging_temp_path(destination);
        match symlink(target, &temporary) {
            Ok(()) => return Ok(temporary),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
}

fn sync_snapshot_directories(snapshots: &[DestinationSnapshot]) -> io::Result<()> {
    let mut directories = std::collections::HashSet::new();
    for snapshot in snapshots {
        if let Some(parent) = snapshot.destination.parent() {
            let key = destination_key(parent);
            if directories.insert(key) {
                sync_parent_directory(&snapshot.destination)?;
            }
        }
    }
    Ok(())
}

#[cfg(not(windows))]
fn replace_file(source: &Path, destination: &Path) -> io::Result<()> {
    fs::rename(source, destination)
}

#[cfg(windows)]
fn replace_file(source: &Path, destination: &Path) -> io::Result<()> {
    const ERROR_FILE_NOT_FOUND: i32 = 2;
    const ERROR_ACCESS_DENIED: i32 = 5;
    const ERROR_SHARING_VIOLATION: i32 = 32;
    const ERROR_UNABLE_TO_REMOVE_REPLACED: i32 = 1175;
    const ERROR_UNABLE_TO_MOVE_REPLACEMENT: i32 = 1176;
    const REPLACE_ATTEMPTS: usize = 50;

    for attempt in 0..REPLACE_ATTEMPTS {
        if fs::symlink_metadata(destination).is_err() {
            match fs::rename(source, destination) {
                Ok(()) => return Ok(()),
                Err(_) if !source.exists() && destination.exists() => return Ok(()),
                Err(error) if fs::symlink_metadata(destination).is_err() => {
                    if attempt + 1 == REPLACE_ATTEMPTS {
                        return Err(error);
                    }
                    std::thread::sleep(Duration::from_millis(1));
                    continue;
                }
                Err(_) => {}
            }
        }

        let (replacement_path, replaced_path) =
            match (fs::canonicalize(source), fs::canonicalize(destination)) {
                (Ok(replacement), Ok(replaced)) => (replacement, replaced),
                (source_result, destination_result) => {
                    if !source.exists() && destination.exists() {
                        return Ok(());
                    }
                    let error = source_result
                        .err()
                        .or_else(|| destination_result.err())
                        .expect("one canonical path failed");
                    if attempt + 1 == REPLACE_ATTEMPTS {
                        return Err(error);
                    }
                    std::thread::sleep(Duration::from_millis(1));
                    continue;
                }
            };
        let replacement = replacement_path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        let replaced = replaced_path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();

        // SAFETY: both path buffers are null-terminated and live for the call.
        let result = unsafe {
            ReplaceFileW(
                replaced.as_ptr(),
                replacement.as_ptr(),
                std::ptr::null(),
                0,
                std::ptr::null(),
                std::ptr::null(),
            )
        };
        if result != 0 {
            return Ok(());
        }

        let error = io::Error::last_os_error();
        if !source.exists() && destination.exists() {
            return Ok(());
        }
        let retryable = matches!(
            error.raw_os_error(),
            Some(
                ERROR_FILE_NOT_FOUND
                    | ERROR_ACCESS_DENIED
                    | ERROR_SHARING_VIOLATION
                    | ERROR_UNABLE_TO_REMOVE_REPLACED
                    | ERROR_UNABLE_TO_MOVE_REPLACEMENT
            )
        );
        if !retryable || attempt + 1 == REPLACE_ATTEMPTS {
            return Err(error);
        }
        std::thread::sleep(Duration::from_millis(1));
    }

    unreachable!("replacement attempts always return")
}

#[cfg(not(windows))]
fn sync_parent_directory(path: &Path) -> io::Result<()> {
    path.parent()
        .map_or(Ok(()), |parent| File::open(parent)?.sync_all())
}

#[cfg(windows)]
fn sync_parent_directory(_path: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::stage_windows_runtime_file;
    use std::fs;

    #[test]
    fn replaces_a_source_hard_link_with_an_independent_runtime_copy() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let source = temp.path().join("native.dll");
        let destination = temp.path().join("target.dll");
        fs::write(&source, b"native").expect("native DLL");
        fs::hard_link(&source, &destination).expect("upstream hard link");

        stage_windows_runtime_file(&source, &destination).expect("stage runtime DLL");
        fs::write(&destination, b"staged").expect("replace staged bytes");

        assert_eq!(fs::read(&source).expect("source DLL"), b"native");
        assert_eq!(fs::read(&destination).expect("staged DLL"), b"staged");
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::{
        acquire_profile_staging_lock, prepare_real_hard_link, publish_all, publish_all_with,
        stage_linux_shared_libraries,
    };
    use std::fs;
    use std::io;
    use std::os::unix::fs::symlink;
    use std::path::{Path, PathBuf};

    #[test]
    fn replaces_dangling_links_in_every_cargo_runtime_directory() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let core_dir = temp.path().join("native/lib");
        let backend_dir = temp.path().join("native/backends");
        let profile_dir = temp.path().join("target/debug");
        fs::create_dir_all(&core_dir).expect("core directory");
        fs::create_dir_all(&backend_dir).expect("backend directory");
        fs::write(core_dir.join("libggml.so.0.13.1"), b"ggml").expect("versioned library");
        symlink("libggml.so.0.13.1", core_dir.join("libggml.so.0")).expect("soname link");
        symlink("libggml.so.0", core_dir.join("libggml.so")).expect("linker-name link");
        fs::write(backend_dir.join("libggml-cpu-x64.so"), b"cpu").expect("CPU module");
        fs::write(backend_dir.join("libggml-vulkan.so"), b"vulkan").expect("Vulkan module");
        fs::write(core_dir.join("libllama-common.so.0.0.0"), b"common")
            .expect("build-support library");
        symlink(
            "libllama-common.so.0.0.0",
            core_dir.join("libllama-common.so"),
        )
        .expect("build-support linker-name link");
        fs::write(core_dir.join("libextra.so"), b"extra").expect("unowned shared library");

        let runtime_sources = [
            core_dir.join("libggml.so"),
            core_dir.join("libggml.so.0"),
            core_dir.join("libggml.so.0.13.1"),
            backend_dir.join("libggml-cpu-x64.so"),
            backend_dir.join("libggml-vulkan.so"),
        ];
        let runtime_sources = runtime_sources
            .iter()
            .map(|path| path.as_path())
            .collect::<Vec<_>>();
        let build_support_source = core_dir.join("libllama-common.so");

        for destination_dir in [
            profile_dir.clone(),
            profile_dir.join("deps"),
            profile_dir.join("examples"),
        ] {
            fs::create_dir_all(&destination_dir).expect("runtime directory");
            symlink("libggml.so.0", destination_dir.join("libggml.so"))
                .expect("upstream dangling link");
            if destination_dir != profile_dir {
                symlink(
                    "libllama-common.so.0",
                    destination_dir.join("libllama-common.so"),
                )
                .expect("upstream non-runtime dangling link");
            }
            symlink("libextra.so.0", destination_dir.join("libextra.so"))
                .expect("unowned dangling link");
            assert!(!destination_dir.join("libggml.so").exists());
            assert!(fs::symlink_metadata(destination_dir.join("libggml.so")).is_ok());
        }

        stage_linux_shared_libraries(
            &runtime_sources,
            &[build_support_source.as_path()],
            &profile_dir,
        )
        .expect("stage shared libraries");
        assert_runtime_sources_unchanged(&runtime_sources);

        for destination_dir in [
            profile_dir.clone(),
            profile_dir.join("deps"),
            profile_dir.join("examples"),
        ] {
            for name in ["libggml.so", "libggml.so.0", "libggml.so.0.13.1"] {
                let destination = destination_dir.join(name);
                assert!(destination.is_file(), "{}", destination.display());
                assert!(
                    !fs::symlink_metadata(&destination)
                        .expect("staged metadata")
                        .file_type()
                        .is_symlink(),
                    "{}",
                    destination.display()
                );
                assert_eq!(fs::read(destination).expect("staged bytes"), b"ggml");
            }
            assert_eq!(
                fs::read(destination_dir.join("libggml-cpu-x64.so")).expect("staged CPU module"),
                b"cpu"
            );
            assert_eq!(
                fs::read(destination_dir.join("libggml-vulkan.so")).expect("staged Vulkan module"),
                b"vulkan"
            );
            assert!(!destination_dir.join("libextra.so").exists());
            assert!(
                fs::symlink_metadata(destination_dir.join("libextra.so"))
                    .expect("unowned link remains")
                    .file_type()
                    .is_symlink(),
                "unowned shared library must remain untouched"
            );
            if destination_dir == profile_dir {
                assert!(
                    fs::symlink_metadata(destination_dir.join("libllama-common.so")).is_err(),
                    "absent build-support entry must remain absent"
                );
            } else {
                assert_eq!(
                    fs::read(destination_dir.join("libllama-common.so"))
                        .expect("repaired upstream build-support link"),
                    b"common"
                );
                assert!(
                    !fs::symlink_metadata(destination_dir.join("libllama-common.so"))
                        .expect("build-support metadata")
                        .file_type()
                        .is_symlink(),
                    "upstream build-support link must resolve to a regular file"
                );
            }
        }

        fs::remove_file(core_dir.join("libllama-common.so.0.0.0"))
            .expect("replace build-support source inode");
        fs::write(core_dir.join("libllama-common.so.0.0.0"), b"common-v2")
            .expect("updated build-support library");
        for destination_dir in [profile_dir.join("deps"), profile_dir.join("examples")] {
            assert_eq!(
                fs::read(destination_dir.join("libllama-common.so"))
                    .expect("stale build-support entry"),
                b"common"
            );
        }

        stage_linux_shared_libraries(
            &runtime_sources,
            &[build_support_source.as_path()],
            &profile_dir,
        )
        .expect("repeated staging remains idempotent");
        assert_runtime_sources_unchanged(&runtime_sources);
        assert!(
            fs::symlink_metadata(profile_dir.join("libllama-common.so")).is_err(),
            "repeated staging must not introduce absent build support"
        );
        for destination_dir in [profile_dir.join("deps"), profile_dir.join("examples")] {
            assert_eq!(
                fs::read(destination_dir.join("libllama-common.so"))
                    .expect("refreshed build-support entry"),
                b"common-v2"
            );
        }
    }

    fn assert_runtime_sources_unchanged(runtime_sources: &[&Path]) {
        let expected: [&[u8]; 5] = [b"ggml", b"ggml", b"ggml", b"cpu", b"vulkan"];
        for (source, expected) in runtime_sources.iter().zip(expected) {
            assert_eq!(
                fs::read(source).expect("upstream runtime source remains readable"),
                expected,
                "staging mutated upstream runtime source {}",
                source.display()
            );
        }
    }

    #[test]
    fn rejects_a_directory_at_a_library_destination() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let core_dir = temp.path().join("native/lib");
        let profile_dir = temp.path().join("target/debug");
        fs::create_dir_all(&core_dir).expect("core directory");
        fs::write(core_dir.join("libllama.so"), b"llama").expect("shared library");
        fs::create_dir_all(profile_dir.join("libllama.so")).expect("conflicting directory");

        let source = core_dir.join("libllama.so");
        let error = stage_linux_shared_libraries(&[source.as_path()], &[], &profile_dir)
            .expect_err("directory collision must fail closed");
        assert_eq!(error.kind(), io::ErrorKind::IsADirectory);
    }

    #[test]
    fn preparation_failure_preserves_every_existing_destination() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let core_dir = temp.path().join("native/lib");
        let profile_dir = temp.path().join("target/debug");
        fs::create_dir_all(&core_dir).expect("core directory");
        fs::create_dir_all(&profile_dir).expect("profile directory");
        fs::write(core_dir.join("liba.so"), b"new-a").expect("first source");
        fs::write(core_dir.join("libb.so"), b"new-b").expect("second source");
        fs::write(profile_dir.join("liba.so"), b"old-a").expect("first destination");
        fs::create_dir(profile_dir.join("libb.so")).expect("second destination collision");

        let sources = [core_dir.join("liba.so"), core_dir.join("libb.so")];
        let source_refs = sources.iter().map(PathBuf::as_path).collect::<Vec<_>>();
        let error = stage_linux_shared_libraries(&source_refs, &[], &profile_dir)
            .expect_err("preparation must fail before publishing any destination");

        assert_eq!(error.kind(), io::ErrorKind::IsADirectory);
        assert_eq!(
            fs::read(profile_dir.join("liba.so")).expect("old destination remains"),
            b"old-a"
        );
        assert_no_staging_temps(&profile_dir);
    }

    #[test]
    fn prepared_link_keeps_old_destination_until_atomic_publish() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let source = temp.path().join("libllama.so");
        let destination = temp.path().join("target.so");
        fs::write(&source, b"new").expect("source");
        fs::write(&destination, b"old").expect("destination");

        let prepared =
            prepare_real_hard_link(&source, &destination).expect("prepare replacement link");
        assert_eq!(
            fs::read(&destination).expect("destination before publish"),
            b"old"
        );
        publish_all(vec![prepared]).expect("publish replacement link");
        assert_eq!(
            fs::read(&destination).expect("destination after publish"),
            b"new"
        );
        assert_no_staging_temps(temp.path());
    }

    #[test]
    fn dropped_prepared_link_cleans_temp_and_preserves_destination() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let source = temp.path().join("libllama.so");
        let destination = temp.path().join("target.so");
        fs::write(&source, b"new").expect("source");
        fs::write(&destination, b"old").expect("destination");

        let prepared =
            prepare_real_hard_link(&source, &destination).expect("prepare replacement link");
        drop(prepared);

        assert_eq!(
            fs::read(&destination).expect("destination after cancellation"),
            b"old"
        );
        assert_no_staging_temps(temp.path());
    }

    #[test]
    fn duplicate_destinations_fail_before_publication() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let first_source = temp.path().join("libfirst.so");
        let second_source = temp.path().join("libsecond.so");
        let destination = temp.path().join("target.so");
        fs::write(&first_source, b"first").expect("first source");
        fs::write(&second_source, b"second").expect("second source");
        fs::write(&destination, b"old").expect("destination");
        let first = prepare_real_hard_link(&first_source, &destination).expect("first replacement");
        let second =
            prepare_real_hard_link(&second_source, &destination).expect("second replacement");

        let error = publish_all(vec![first, second]).expect_err("duplicate destination");

        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(
            fs::read(&destination).expect("destination after rejection"),
            b"old"
        );
        assert_no_staging_temps(temp.path());
    }

    #[test]
    fn late_publication_failure_restores_the_complete_previous_batch() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let first_source = temp.path().join("libfirst.so");
        let second_source = temp.path().join("libsecond.so");
        let first_destination = temp.path().join("first.so");
        let second_destination = temp.path().join("second.so");
        fs::write(&first_source, b"new-first").expect("first source");
        fs::write(&second_source, b"new-second").expect("second source");
        fs::write(&first_destination, b"old-first").expect("first destination");
        fs::write(&second_destination, b"old-second").expect("second destination");
        let first =
            prepare_real_hard_link(&first_source, &first_destination).expect("first replacement");
        let second = prepare_real_hard_link(&second_source, &second_destination)
            .expect("second replacement");
        let mut attempt = 0;

        let error = publish_all_with(vec![first, second], |source, destination| {
            attempt += 1;
            if attempt == 2 {
                return Err(io::Error::other("injected late publication failure"));
            }
            super::replace_file(source, destination)
        })
        .expect_err("second publication must fail");

        assert_eq!(error.to_string(), "injected late publication failure");
        assert_eq!(
            fs::read(&first_destination).expect("restored first destination"),
            b"old-first"
        );
        assert_eq!(
            fs::read(&second_destination).expect("unchanged second destination"),
            b"old-second"
        );
        assert_no_staging_temps(temp.path());
    }

    #[test]
    fn late_publication_failure_restores_a_previous_symlink() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let old_source = temp.path().join("libold.so");
        let first_source = temp.path().join("libfirst.so");
        let second_source = temp.path().join("libsecond.so");
        let first_destination = temp.path().join("first.so");
        let second_destination = temp.path().join("second.so");
        fs::write(&old_source, b"old-first").expect("old source");
        fs::write(&first_source, b"new-first").expect("first source");
        fs::write(&second_source, b"new-second").expect("second source");
        symlink("libold.so", &first_destination).expect("previous destination symlink");
        fs::write(&second_destination, b"old-second").expect("second destination");
        let first =
            prepare_real_hard_link(&first_source, &first_destination).expect("first replacement");
        let second = prepare_real_hard_link(&second_source, &second_destination)
            .expect("second replacement");
        let mut attempt = 0;

        publish_all_with(vec![first, second], |source, destination| {
            attempt += 1;
            if attempt == 2 {
                return Err(io::Error::other("injected late publication failure"));
            }
            super::replace_file(source, destination)
        })
        .expect_err("second publication must fail");

        assert_eq!(
            fs::read_link(&first_destination).expect("restored symlink"),
            Path::new("libold.so")
        );
        assert_eq!(
            fs::read(&first_destination).expect("restored symlink content"),
            b"old-first"
        );
        assert_no_staging_temps(temp.path());
    }

    #[test]
    fn profile_lock_serializes_concurrent_staging() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let source = temp.path().join("native/libllama.so");
        let profile_dir = temp.path().join("target/debug");
        fs::create_dir_all(source.parent().expect("source parent")).expect("source directory");
        fs::write(&source, b"llama").expect("source");
        let lock = acquire_profile_staging_lock(&profile_dir).expect("hold profile lock");
        let (sender, receiver) = std::sync::mpsc::channel();
        let thread_source = source.clone();
        let thread_profile = profile_dir.clone();
        let worker = std::thread::spawn(move || {
            let result =
                stage_linux_shared_libraries(&[thread_source.as_path()], &[], &thread_profile);
            sender.send(result).expect("send staging result");
        });

        assert!(
            matches!(
                receiver.recv_timeout(std::time::Duration::from_millis(50)),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout)
            ),
            "concurrent staging must wait for the profile lock"
        );
        drop(lock);
        receiver
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("staging resumes after lock release")
            .expect("staging succeeds");
        worker.join().expect("staging worker");
        assert_eq!(
            fs::read(profile_dir.join("libllama.so")).expect("staged destination"),
            b"llama"
        );
    }

    fn assert_no_staging_temps(directory: &Path) {
        let names = fs::read_dir(directory)
            .expect("read staging directory")
            .map(|entry| {
                entry
                    .expect("staging entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect::<Vec<_>>();
        assert!(
            names.iter().all(|name| !name.contains(".codestory-stage-")),
            "staging temporaries remain: {names:?}"
        );
    }
}
