use codestory_contracts::bounded_locks::{
    FileLockKind, LockDeadline, PUBLICATION_LOCK_WAIT, acquire_with_deadline,
};
use sha2::{Digest, Sha256};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;
#[cfg(windows)]
use std::time::Duration;
#[cfg(windows)]
use windows_sys::Win32::Storage::FileSystem::ReplaceFileW;

const UPSTREAM_BUILD_SUPPORT_LIBRARY_NAMES: &[&str] = &["libllama-common.so"];
pub(crate) const NATIVE_SEEDS_DIR: &str = ".codestory-native-seeds";
pub(crate) const NATIVE_RUNTIME_FILE_LIST: &str = "codestory-native-runtime-files-v1.txt";
/// Infix and suffix that make a staging residue name exclusively CodeStory's.
/// Everything between them is the attempt identity; a name that does not parse
/// completely is foreign and is never inspected, moved, or removed.
const STAGING_RESIDUE_INFIX: &str = ".codestory-stage-";
const STAGING_RESIDUE_SUFFIX: &str = ".tmp";
static STAGING_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
pub(crate) struct StagedNativeGeneration {
    pub(crate) id: String,
    pub(crate) directory: PathBuf,
    pub(crate) runtime_file_names: Vec<String>,
}

/// Stage the complete ABI-coupled runtime in an immutable directory.
///
/// This deliberately does not activate the generation. The final executable
/// does not exist while this build script runs, so publishing here would expose
/// an incomplete generation. The CLI launcher adds its matching runtime
/// executable and atomically publishes the one generation pointer.
pub(crate) fn stage_native_generation(
    sources: &[&Path],
    profile_dir: &Path,
) -> io::Result<StagedNativeGeneration> {
    let mut entries = sources
        .iter()
        .map(|source| {
            let name = source
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!(
                            "native runtime file name is not UTF-8: {}",
                            source.display()
                        ),
                    )
                })?
                .to_owned();
            validate_runtime_file_name(&name)?;
            let source = fs::canonicalize(source)?;
            validate_regular_source(&source, "native runtime")?;
            Ok((name, source))
        })
        .collect::<io::Result<Vec<_>>>()?;
    entries.sort_by_key(|entry| entry.0.to_lowercase());
    for pair in entries.windows(2) {
        if pair[0].0.eq_ignore_ascii_case(&pair[1].0) {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("duplicate native runtime artifact: {}", pair[0].0),
            ));
        }
    }

    let id = generation_id(&entries)?;
    let seeds_dir = profile_dir.join(NATIVE_SEEDS_DIR);
    let generation_dir = seeds_dir.join(&id);
    fs::create_dir_all(&seeds_dir)?;
    let _lock = acquire_profile_staging_lock(profile_dir)?;

    if generation_dir.exists() {
        verify_generation(&generation_dir, &entries)?;
    } else {
        let temporary = seeds_dir.join(residue_file_name(
            &id,
            StagingRole::Seed,
            StagingAttempt::next(),
        ));
        fs::create_dir(&temporary)?;
        let result = (|| {
            for (name, source) in &entries {
                copy_verified(source, &temporary.join(name))?;
            }
            write_runtime_file_list(
                &temporary.join(NATIVE_RUNTIME_FILE_LIST),
                entries.iter().map(|(name, _)| name.as_str()),
            )?;
            sync_directory(&temporary)?;
            match fs::rename(&temporary, &generation_dir) {
                Ok(()) => sync_parent_directory(&generation_dir)?,
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::AlreadyExists | io::ErrorKind::DirectoryNotEmpty
                    ) =>
                {
                    fs::remove_dir_all(&temporary)?;
                    verify_generation(&generation_dir, &entries)?;
                }
                Err(error) => return Err(error),
            }
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_dir_all(&temporary);
        }
        result?;
    }

    Ok(StagedNativeGeneration {
        id,
        directory: generation_dir,
        runtime_file_names: entries.into_iter().map(|(name, _)| name).collect(),
    })
}

fn generation_id(entries: &[(String, PathBuf)]) -> io::Result<String> {
    let mut hasher = Sha256::new();
    hasher.update(b"codestory-native-generation-v1\0");
    for (name, source) in entries {
        hasher.update(name.as_bytes());
        hasher.update([0]);
        hasher.update(file_sha256(source)?);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn verify_generation(generation_dir: &Path, entries: &[(String, PathBuf)]) -> io::Result<()> {
    let names = fs::read_to_string(generation_dir.join(NATIVE_RUNTIME_FILE_LIST))?;
    let expected = entries
        .iter()
        .map(|(name, _)| name.as_str())
        .collect::<Vec<_>>();
    if names.lines().collect::<Vec<_>>() != expected {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "native generation manifest differs from its content identity: {}",
                generation_dir.display()
            ),
        ));
    }
    for (name, source) in entries {
        let destination = generation_dir.join(name);
        validate_regular_source(&destination, "staged native runtime")?;
        if file_sha256(source)? != file_sha256(&destination)? {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "immutable native generation contains unexpected bytes: {}",
                    destination.display()
                ),
            ));
        }
    }
    Ok(())
}

fn copy_verified(source: &Path, destination: &Path) -> io::Result<()> {
    let mut input = File::open(source)?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)?;
    io::copy(&mut input, &mut output)?;
    output.sync_all()?;
    drop(output);
    if file_sha256(source)? != file_sha256(destination)? {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "native runtime copy differs from source: {}",
                source.display()
            ),
        ));
    }
    Ok(())
}

fn validate_runtime_file_name(name: &str) -> io::Result<()> {
    if name.is_empty()
        || matches!(name, "." | "..")
        || name.contains(['/', '\\'])
        || Path::new(name).file_name().and_then(|value| value.to_str()) != Some(name)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("unsafe native runtime file name: {name:?}"),
        ));
    }
    Ok(())
}

fn write_runtime_file_list<'a>(
    destination: &Path,
    names: impl IntoIterator<Item = &'a str>,
) -> io::Result<()> {
    let mut body = names.into_iter().collect::<Vec<_>>().join("\n");
    body.push('\n');
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)?;
    file.write_all(body.as_bytes())?;
    file.sync_all()
}

/// Copy one Windows runtime DLL without exposing a missing or partial destination.
#[cfg(test)]
pub(crate) fn stage_windows_runtime_file(source: &Path, destination: &Path) -> io::Result<()> {
    stage_windows_runtime_files(&[(source, destination)]).map(|_| ())
}

/// Prepare every Windows runtime DLL before publishing the transaction.
///
/// The upstream build can leave hard links in Cargo's runtime directory.
/// Copying to unique same-directory files breaks those links. The publication
/// transaction snapshots every destination before its first replacement and
/// restores the complete previous set if a later replacement fails.
///
/// Process death skips that in-process rollback, so the attempt is classified
/// and settled from disk first, under the same profile lock that proves no
/// other attempt can own the residue found there.
pub(crate) fn stage_windows_runtime_files(
    entries: &[(&Path, &Path)],
) -> io::Result<Vec<ResidueRecovery>> {
    let Some(profile_dir) = entries
        .first()
        .and_then(|(_, destination)| destination.parent())
    else {
        return Ok(Vec::new());
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
    let destination_names = entries
        .iter()
        .map(|(_, destination)| {
            destination
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!(
                            "native runtime destination name is not UTF-8: {}",
                            destination.display()
                        ),
                    )
                })
        })
        .collect::<io::Result<Vec<_>>>()?;
    let _lock = acquire_profile_staging_lock(profile_dir)?;
    let recovered = recover_staging_residue(profile_dir, &destination_names)?;
    let mut replacements = Vec::with_capacity(entries.len());
    for (source, destination) in entries {
        replacements.push(prepare_runtime_copy(source, destination)?);
    }
    publish_all(replacements)?;
    Ok(recovered)
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
/// target directory cannot retain bytes from an older dependency build. All
/// destinations are prepared first and published under one rollback-capable
/// profile transaction.
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
    let temporary = create_unique_hard_link(&source, destination, StagingRole::Copy)?;
    Ok(PreparedReplacement::new(
        temporary,
        destination,
        "shared-library",
        true,
    ))
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
    Ok(PreparedReplacement::new(
        temporary,
        destination,
        "native runtime",
        false,
    ))
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
        let temporary = staging_temp_path(destination, StagingRole::Copy);
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

fn create_unique_hard_link(
    source: &Path,
    destination: &Path,
    role: StagingRole,
) -> io::Result<PathBuf> {
    loop {
        let temporary = staging_temp_path(destination, role);
        match fs::hard_link(source, &temporary) {
            Ok(()) => return Ok(temporary),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
}

/// Role every CodeStory staging residue records in its own name.
///
/// The role is what makes crash recovery a decision instead of a guess: a
/// `Copy` never holds bytes that exist nowhere else, while a `Restore` is the
/// only remaining witness of the runtime file its attempt was about to replace.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum StagingRole {
    /// Independent copy of a new runtime file, not yet published.
    Copy,
    /// Snapshot of the destination taken before its first replacement.
    Restore,
    /// Immutable-generation staging directory under the seeds directory.
    Seed,
}

impl StagingRole {
    fn token(self) -> &'static str {
        match self {
            Self::Copy => "copy",
            Self::Restore => "restore",
            Self::Seed => "seed",
        }
    }

    fn from_token(token: &str) -> Option<Self> {
        match token {
            "copy" => Some(Self::Copy),
            "restore" => Some(Self::Restore),
            "seed" => Some(Self::Seed),
            _ => None,
        }
    }
}

impl fmt::Display for StagingRole {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.token())
    }
}

/// Identity of one staging attempt, embedded in every residue name it creates.
///
/// `pid` alone cannot own residue: the operating system reuses process ids
/// across a crash or reboot, so a later process could claim a dead process's
/// files. Pairing the pid with the creating process's start epoch makes the
/// identity exact — residue carrying this process's pid but an epoch older
/// than this process incarnation provably belongs to a dead process.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct StagingAttempt {
    pid: u32,
    epoch_nanos: u128,
    sequence: u64,
}

impl StagingAttempt {
    fn next() -> Self {
        Self {
            pid: std::process::id(),
            epoch_nanos: process_epoch_nanos(),
            sequence: STAGING_COUNTER.fetch_add(1, Ordering::Relaxed),
        }
    }

    /// Whether no live attempt in this process incarnation can own the residue.
    fn is_abandoned(self) -> bool {
        self.pid != std::process::id() || self.epoch_nanos < process_epoch_nanos()
    }
}

impl fmt::Display for StagingAttempt {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}-{}-{}",
            self.pid, self.epoch_nanos, self.sequence
        )
    }
}

/// Wall-clock start marker of this process incarnation, captured once.
///
/// A clock that cannot be read at all yields `0`, which classifies every
/// same-pid residue as live and therefore untouchable. Recovery then does
/// nothing rather than removing something it cannot prove it owns.
fn process_epoch_nanos() -> u128 {
    static PROCESS_EPOCH: OnceLock<u128> = OnceLock::new();
    *PROCESS_EPOCH.get_or_init(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos())
    })
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct StagingResidue {
    destination_name: String,
    role: StagingRole,
    attempt: StagingAttempt,
}

fn residue_file_name(destination_name: &str, role: StagingRole, attempt: StagingAttempt) -> String {
    format!(".{destination_name}{STAGING_RESIDUE_INFIX}{role}-{attempt}{STAGING_RESIDUE_SUFFIX}")
}

/// Parse a directory entry name back into the attempt that owns it.
///
/// Every component must round-trip the exact formatting `residue_file_name`
/// produces. A truncated, padded, or extended identity yields `None`, which
/// keeps the name foreign and therefore immune to recovery.
fn parse_staging_residue(file_name: &str) -> Option<StagingResidue> {
    let body = file_name
        .strip_prefix('.')?
        .strip_suffix(STAGING_RESIDUE_SUFFIX)?;
    let (destination_name, identity) = body.rsplit_once(STAGING_RESIDUE_INFIX)?;
    if destination_name.is_empty() {
        return None;
    }
    let mut parts = identity.split('-');
    let role = StagingRole::from_token(parts.next()?)?;
    let pid = parse_canonical_decimal::<u32>(parts.next()?)?;
    let epoch_nanos = parse_canonical_decimal::<u128>(parts.next()?)?;
    let sequence = parse_canonical_decimal::<u64>(parts.next()?)?;
    if parts.next().is_some() {
        return None;
    }
    Some(StagingResidue {
        destination_name: destination_name.to_owned(),
        role,
        attempt: StagingAttempt {
            pid,
            epoch_nanos,
            sequence,
        },
    })
}

fn parse_canonical_decimal<T: std::str::FromStr>(part: &str) -> Option<T> {
    if part.is_empty()
        || !part.bytes().all(|byte| byte.is_ascii_digit())
        || (part.len() > 1 && part.starts_with('0'))
    {
        return None;
    }
    part.parse().ok()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RecoveryDecision {
    /// Residue whose bytes exist elsewhere; the destination was already whole.
    Discarded,
    /// The interrupted attempt's rollback, completed from its own snapshot.
    RestoredPreviousRuntime,
}

impl fmt::Display for RecoveryDecision {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Discarded => "discarded",
            Self::RestoredPreviousRuntime => "restored-previous-runtime",
        })
    }
}

/// Attributable record of one recovery decision.
#[derive(Debug)]
pub(crate) struct ResidueRecovery {
    residue: PathBuf,
    destination: PathBuf,
    role: StagingRole,
    attempt: StagingAttempt,
    decision: RecoveryDecision,
}

impl fmt::Display for ResidueRecovery {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "native staging recovery: {} residue {} owned by attempt {} for destination {} was {}",
            self.role,
            self.residue.display(),
            self.attempt,
            self.destination.display(),
            self.decision
        )
    }
}

/// Classify and settle every abandoned staging attempt in one directory.
///
/// Callers must already hold the profile staging lock, which is what proves a
/// matching residue cannot belong to a concurrent attempt in another process.
/// This settles the independent-copy path only, where a destination is always
/// a regular file, so a link found at one is a swap rather than a layout.
///
/// A `Restore` snapshot is always rolled back onto its destination rather than
/// only when the destination is missing. A crashed attempt cannot be asked
/// whether it finished, so the only provable terminal state is the complete
/// previous runtime; the caller re-stages immediately afterwards, so restoring
/// a transaction that had in fact completed costs one repeated publication and
/// never a torn set.
pub(crate) fn recover_staging_residue(
    directory: &Path,
    destination_names: &[&str],
) -> io::Result<Vec<ResidueRecovery>> {
    for name in destination_names {
        validate_runtime_file_name(name)?;
    }
    let owned = destination_names
        .iter()
        .map(|name| (destination_key(Path::new(name)), (*name).to_owned()))
        .collect::<std::collections::BTreeMap<_, _>>();
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };

    let mut abandoned = Vec::new();
    for entry in entries {
        let entry = entry?;
        let file_name = entry.file_name();
        let Some(file_name) = file_name.to_str() else {
            continue;
        };
        let Some(residue) = parse_staging_residue(file_name) else {
            continue;
        };
        // Seed residue is an immutable-generation directory under the seeds
        // directory, never a runtime file beside a destination.
        if residue.role == StagingRole::Seed {
            continue;
        }
        let Some(destination_name) =
            owned.get(&destination_key(Path::new(&residue.destination_name)))
        else {
            continue;
        };
        if !residue.attempt.is_abandoned() {
            continue;
        }
        abandoned.push((
            directory.join(destination_name),
            directory.join(file_name),
            residue,
        ));
    }
    abandoned.sort_by(|left, right| {
        (&left.0, left.2.role, left.2.attempt).cmp(&(&right.0, right.2.role, right.2.attempt))
    });

    for window in abandoned.windows(2) {
        if window[0].2.role == StagingRole::Restore
            && window[1].2.role == StagingRole::Restore
            && window[0].0 == window[1].0
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "native runtime destination {} has more than one owned restore snapshot ({} and {}); recovery cannot prove which one is the previous runtime",
                    window[0].0.display(),
                    window[0].1.display(),
                    window[1].1.display()
                ),
            ));
        }
    }
    for (_, residue_path, _) in &abandoned {
        validate_owned_residue(residue_path)?;
    }

    let mut recoveries = Vec::with_capacity(abandoned.len());
    for (destination, residue_path, residue) in abandoned {
        let decision = match residue.role {
            StagingRole::Restore => {
                validate_destination(&destination, "native runtime", false)?;
                // The snapshot is a hard link to the destination until the
                // first replacement lands, so an unstarted or already rolled
                // back attempt leaves the two identical. Replacing a file with
                // its own link is a no-op that would strand the residue.
                if destination_holds(&destination, &residue_path)? {
                    fs::remove_file(&residue_path)?;
                    RecoveryDecision::Discarded
                } else {
                    replace_file(&residue_path, &destination)?;
                    RecoveryDecision::RestoredPreviousRuntime
                }
            }
            // A prepared copy never holds the only instance of its bytes, and
            // seed residue is filtered out above, so neither can strand a
            // runtime file by being removed.
            StagingRole::Copy | StagingRole::Seed => {
                fs::remove_file(&residue_path)?;
                RecoveryDecision::Discarded
            }
        };
        recoveries.push(ResidueRecovery {
            residue: residue_path,
            destination,
            role: residue.role,
            attempt: residue.attempt,
            decision,
        });
    }
    if recoveries
        .iter()
        .any(|recovery| recovery.decision == RecoveryDecision::RestoredPreviousRuntime)
    {
        sync_destination_directories(
            recoveries
                .iter()
                .map(|recovery| recovery.destination.as_path()),
        )?;
    }
    Ok(recoveries)
}

/// Whether the destination already holds exactly the snapshot's bytes.
///
/// The caller has already proven the destination is absent or a regular file.
fn destination_holds(destination: &Path, snapshot: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(destination) {
        Ok(_) => Ok(file_sha256(destination)? == file_sha256(snapshot)?),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

/// Reject anything wearing an owned residue name that is not a plain file.
///
/// A directory or reparse point at an exclusively CodeStory name means the
/// name was taken by something else, so recovery stops before it could delete
/// a directory tree or follow a link out of the profile directory.
fn validate_owned_residue(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    let file_type = metadata.file_type();
    if file_type.is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "owned native staging residue is a reparse point: {}",
                path.display()
            ),
        ));
    }
    if file_type.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::IsADirectory,
            format!(
                "owned native staging residue is a directory: {}",
                path.display()
            ),
        ));
    }
    if !file_type.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "owned native staging residue is not a regular file: {}",
                path.display()
            ),
        ));
    }
    Ok(())
}

fn staging_temp_path(destination: &Path, role: StagingRole) -> PathBuf {
    let name = destination
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_else(|| "native-runtime".into());
    destination.with_file_name(residue_file_name(&name, role, StagingAttempt::next()))
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
    // A sibling build staging the same profile holds this for a full runtime
    // tree copy and verify. Bounded rather than blocking so a wedged sibling
    // cannot hang the build forever.
    acquire_with_deadline(
        &lock,
        FileLockKind::Exclusive,
        LockDeadline::after(PUBLICATION_LOCK_WAIT),
        None,
    )
    .map_err(|error| io::Error::other(error.to_string()))?;
    Ok(lock)
}

fn publish_all(replacements: Vec<PreparedReplacement>) -> io::Result<()> {
    publish_all_with(replacements, replace_file)
}

/// Reproduce process death partway through a publication transaction.
///
/// Leaking the transaction and its prepared replacements skips every `Drop`,
/// which is precisely what a killed process leaves on disk: owned residue with
/// no in-process rollback and no cleanup.
#[cfg(test)]
fn simulate_publication_death(
    mut replacements: Vec<PreparedReplacement>,
    published_before_death: usize,
) -> io::Result<()> {
    let transaction = PublicationTransaction::capture(&replacements)?;
    let mut publish = replace_file;
    for replacement in replacements.iter_mut().take(published_before_death) {
        replacement.publish_with(&mut publish)?;
    }
    std::mem::forget(transaction);
    std::mem::forget(replacements);
    Ok(())
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
    if let Err(error) = sync_destination_directories(
        replacements
            .iter()
            .map(|replacement| replacement.destination.as_path()),
    ) {
        return Err(transaction.rollback(replacements.len(), error));
    }
    Ok(())
}

fn sync_destination_directories<'a>(
    destinations: impl IntoIterator<Item = &'a Path>,
) -> io::Result<()> {
    let mut directories = std::collections::HashSet::new();
    for destination in destinations {
        if let Some(parent) = destination.parent() {
            let key = destination_key(parent);
            if directories.insert(key) {
                sync_parent_directory(destination)?;
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
    label: &'static str,
    allow_symlink: bool,
}

impl PreparedReplacement {
    fn new(
        temporary: PathBuf,
        destination: &Path,
        label: &'static str,
        allow_symlink: bool,
    ) -> Self {
        Self {
            temporary: Some(temporary),
            destination: destination.to_path_buf(),
            label,
            allow_symlink,
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
        // Preparation validated this path, but a directory or reparse point
        // swapped in afterwards would otherwise be replaced silently.
        validate_destination(&self.destination, self.label, self.allow_symlink)?;
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
        if let Err(error) = sync_destination_directories(
            self.snapshots[..published_count]
                .iter()
                .map(|snapshot| snapshot.destination.as_path()),
        ) {
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
            Ok(metadata) if metadata.is_file() => PreviousDestination::File(
                create_unique_hard_link(destination, destination, StagingRole::Restore)?,
            ),
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
        let temporary = staging_temp_path(destination, StagingRole::Restore);
        match symlink(target, &temporary) {
            Ok(()) => return Ok(temporary),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
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
fn sync_directory(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()
}

#[cfg(windows)]
fn sync_directory(_path: &Path) -> io::Result<()> {
    Ok(())
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

#[cfg(test)]
mod generation_tests {
    use super::{NATIVE_RUNTIME_FILE_LIST, NATIVE_SEEDS_DIR, stage_native_generation};
    use std::fs;
    use std::path::PathBuf;

    #[test]
    fn stages_complete_immutable_native_seeds() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let sources = temp.path().join("sources");
        let profile = temp.path().join("target/debug");
        fs::create_dir_all(&sources).expect("source directory");
        fs::write(sources.join("libggml.so"), b"generation-one").expect("first source");
        fs::write(sources.join("libllama.so"), b"llama").expect("second source");
        let paths = [sources.join("libggml.so"), sources.join("libllama.so")];
        let refs = paths.iter().map(PathBuf::as_path).collect::<Vec<_>>();

        let first = stage_native_generation(&refs, &profile).expect("first generation");
        assert_eq!(
            fs::read_to_string(first.directory.join(NATIVE_RUNTIME_FILE_LIST))
                .expect("generation manifest"),
            "libggml.so\nlibllama.so\n"
        );
        fs::write(&paths[0], b"generation-two").expect("replace source bytes");
        let second = stage_native_generation(&refs, &profile).expect("second generation");
        assert_ne!(first.id, second.id);
        assert_eq!(
            fs::read(first.directory.join("libggml.so")).expect("immutable first bytes"),
            b"generation-one"
        );
        assert_eq!(
            fs::read(second.directory.join("libggml.so")).expect("second bytes"),
            b"generation-two"
        );
        assert_eq!(
            fs::read_dir(profile.join(NATIVE_SEEDS_DIR))
                .expect("seed inventory")
                .count(),
            2
        );
    }
}

/// Ownership, restart classification, and crash recovery for the accepted
/// independent-copy staging path. The state machine is platform neutral, so it
/// is proven on every host rather than only on Windows.
#[cfg(test)]
mod residue_tests {
    use super::{
        RecoveryDecision, StagingAttempt, StagingRole, parse_staging_residue, prepare_runtime_copy,
        publish_all_with, recover_staging_residue, replace_file, residue_file_name,
        simulate_publication_death, stage_windows_runtime_files, staging_temp_path,
    };
    use std::fs;
    use std::io;
    use std::path::{Path, PathBuf};

    /// Residue carrying this process's pid but an older epoch: exactly what a
    /// crashed process leaves behind once the operating system reuses its pid.
    fn abandoned_residue_name(destination_name: &str, role: StagingRole, sequence: u64) -> String {
        residue_file_name(
            destination_name,
            role,
            StagingAttempt {
                pid: std::process::id(),
                epoch_nanos: 1,
                sequence,
            },
        )
    }

    /// Rewrite live residue identities as an earlier process incarnation.
    ///
    /// A simulated death happens inside the running test process, so its
    /// residue is still owned by a live attempt. Ageing the epoch — and only
    /// the epoch — turns the same real files, links, and roles into what a new
    /// process finds after the previous one died and its pid was reused.
    fn age_residue_into_a_dead_incarnation(directory: &Path) {
        for entry in fs::read_dir(directory).expect("read staging directory") {
            let entry = entry.expect("staging entry");
            let file_name = entry.file_name();
            let file_name = file_name.to_string_lossy();
            let Some(residue) = parse_staging_residue(&file_name) else {
                continue;
            };
            let aged = residue_file_name(
                &residue.destination_name,
                residue.role,
                StagingAttempt {
                    epoch_nanos: 1,
                    ..residue.attempt
                },
            );
            fs::rename(entry.path(), directory.join(aged)).expect("age staging residue");
        }
    }

    fn write(path: &Path, bytes: &[u8]) {
        fs::write(path, bytes).unwrap_or_else(|error| panic!("write {}: {error}", path.display()));
    }

    fn read(path: &Path) -> Vec<u8> {
        fs::read(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
    }

    fn entry_names(directory: &Path) -> Vec<String> {
        let mut names = fs::read_dir(directory)
            .expect("read staging directory")
            .map(|entry| {
                entry
                    .expect("staging entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect::<Vec<_>>();
        names.sort();
        names
    }

    fn assert_no_staging_residue(directory: &Path) {
        let names = entry_names(directory);
        assert!(
            names
                .iter()
                .all(|name| !name.contains(super::STAGING_RESIDUE_INFIX)),
            "staging residue remains: {names:?}"
        );
    }

    #[test]
    fn residue_names_round_trip_their_owning_attempt() {
        let attempt = StagingAttempt {
            pid: 4321,
            epoch_nanos: 1_700_000_000_123_456_789,
            sequence: 7,
        };
        let name = residue_file_name("llama.dll", StagingRole::Restore, attempt);
        assert_eq!(
            name,
            ".llama.dll.codestory-stage-restore-4321-1700000000123456789-7.tmp"
        );
        let parsed = parse_staging_residue(&name).expect("owned residue parses");
        assert_eq!(parsed.destination_name, "llama.dll");
        assert_eq!(parsed.role, StagingRole::Restore);
        assert_eq!(parsed.attempt, attempt);
    }

    #[test]
    fn foreign_and_malformed_residue_is_never_classified_as_owned() {
        let owned = abandoned_residue_name("llama.dll", StagingRole::Copy, 3);
        assert!(parse_staging_residue(&owned).is_some());
        for name in [
            // No leading dot: not a CodeStory staging name.
            "llama.dll.codestory-stage-copy-11-22-33.tmp",
            // Truncated identity.
            ".llama.dll.codestory-stage-copy-11-22.tmp",
            // Extended identity.
            ".llama.dll.codestory-stage-copy-11-22-33-44.tmp",
            // Unknown role.
            ".llama.dll.codestory-stage-publish-11-22-33.tmp",
            // Non-numeric identity.
            ".llama.dll.codestory-stage-copy-eleven-22-33.tmp",
            // Zero-padded identity is not the canonical form we write.
            ".llama.dll.codestory-stage-copy-011-22-33.tmp",
            // Empty destination name.
            "..codestory-stage-copy-11-22-33.tmp",
            // Wrong suffix.
            ".llama.dll.codestory-stage-copy-11-22-33.temp",
            // No identity at all.
            ".llama.dll.tmp",
        ] {
            assert!(
                parse_staging_residue(name).is_none(),
                "{name} must stay foreign"
            );
        }
    }

    #[test]
    fn recovery_discards_abandoned_copies_and_leaves_foreign_files_untouched() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let profile = temp.path();
        let destination = profile.join("llama.dll");
        write(&destination, b"previous-runtime");
        let abandoned = profile.join(abandoned_residue_name("llama.dll", StagingRole::Copy, 1));
        write(&abandoned, b"half-written");
        let foreign_lookalike = profile.join(".llama.dll.codestory-stage-copy-eleven-22-33.tmp");
        write(&foreign_lookalike, b"foreign");
        let unrelated_owned = profile.join(abandoned_residue_name(
            "someone-else.dll",
            StagingRole::Copy,
            2,
        ));
        write(&unrelated_owned, b"another package");

        let recovered = recover_staging_residue(profile, &["llama.dll"]).expect("recovery");

        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].role, StagingRole::Copy);
        assert_eq!(recovered[0].decision, RecoveryDecision::Discarded);
        assert_eq!(recovered[0].destination, destination);
        assert_eq!(
            read(&destination),
            b"previous-runtime",
            "the previous complete runtime must stay usable"
        );
        assert!(!abandoned.exists());
        assert_eq!(read(&foreign_lookalike), b"foreign");
        assert_eq!(read(&unrelated_owned), b"another package");
    }

    #[test]
    fn recovery_rolls_an_abandoned_snapshot_back_onto_a_missing_destination() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let profile = temp.path();
        let destination = profile.join("llama.dll");
        let snapshot = profile.join(abandoned_residue_name("llama.dll", StagingRole::Restore, 4));
        write(&snapshot, b"previous-runtime");

        let recovered = recover_staging_residue(profile, &["llama.dll"]).expect("recovery");

        assert_eq!(recovered.len(), 1);
        assert_eq!(
            recovered[0].decision,
            RecoveryDecision::RestoredPreviousRuntime
        );
        assert_eq!(read(&destination), b"previous-runtime");
        assert_no_staging_residue(profile);

        let repeated = recover_staging_residue(profile, &["llama.dll"]).expect("repeated recovery");
        assert!(repeated.is_empty(), "repeated recovery must be idempotent");
        assert_eq!(read(&destination), b"previous-runtime");
    }

    #[test]
    fn a_snapshot_its_destination_already_holds_is_discarded_not_replayed() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let profile = temp.path();
        let destination = profile.join("llama.dll");
        write(&destination, b"previous-runtime");
        let snapshot = profile.join(abandoned_residue_name(
            "llama.dll",
            StagingRole::Restore,
            12,
        ));
        fs::hard_link(&destination, &snapshot).expect("snapshot link");

        let recovered = recover_staging_residue(profile, &["llama.dll"]).expect("recovery");

        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].decision, RecoveryDecision::Discarded);
        assert_eq!(read(&destination), b"previous-runtime");
        assert_no_staging_residue(profile);
    }

    #[test]
    fn recovery_never_settles_residue_owned_by_a_live_attempt() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let profile = temp.path();
        let destination = profile.join("llama.dll");
        write(&destination, b"previous-runtime");
        let live = staging_temp_path(&destination, StagingRole::Copy);
        write(&live, b"in flight");

        let recovered = recover_staging_residue(profile, &["llama.dll"]).expect("recovery");

        assert!(recovered.is_empty());
        assert_eq!(read(&live), b"in flight");
    }

    #[test]
    fn a_directory_wearing_an_owned_residue_name_fails_closed() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let profile = temp.path();
        let destination = profile.join("llama.dll");
        write(&destination, b"previous-runtime");
        let hostile = profile.join(abandoned_residue_name("llama.dll", StagingRole::Copy, 5));
        fs::create_dir(&hostile).expect("hostile directory");
        write(&hostile.join("payload"), b"do not delete me");

        let error = recover_staging_residue(profile, &["llama.dll"])
            .expect_err("a directory must fail before any mutation");

        assert_eq!(error.kind(), io::ErrorKind::IsADirectory);
        assert_eq!(read(&hostile.join("payload")), b"do not delete me");
        assert_eq!(read(&destination), b"previous-runtime");
    }

    #[cfg(unix)]
    #[test]
    fn a_link_wearing_an_owned_residue_name_fails_closed() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let profile = temp.path();
        let destination = profile.join("llama.dll");
        write(&destination, b"previous-runtime");
        let outside = temp.path().join("outside.bin");
        write(&outside, b"outside the profile");
        let hostile = profile.join(abandoned_residue_name("llama.dll", StagingRole::Copy, 6));
        std::os::unix::fs::symlink(&outside, &hostile).expect("hostile link");

        let error = recover_staging_residue(profile, &["llama.dll"])
            .expect_err("a reparse point must fail before any mutation");

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(read(&outside), b"outside the profile");
        assert!(fs::symlink_metadata(&hostile).is_ok());
    }

    #[test]
    fn a_destination_replaced_by_a_reparse_point_fails_before_recovery_mutates_it() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let profile = temp.path();
        let destination = profile.join("llama.dll");
        let snapshot = profile.join(abandoned_residue_name("llama.dll", StagingRole::Restore, 7));
        write(&snapshot, b"previous-runtime");
        fs::create_dir(&destination).expect("path swapped to a directory");

        let error = recover_staging_residue(profile, &["llama.dll"])
            .expect_err("a swapped destination must fail before rollback");

        assert_eq!(error.kind(), io::ErrorKind::IsADirectory);
        assert!(destination.is_dir());
        assert_eq!(read(&snapshot), b"previous-runtime");
    }

    #[test]
    fn two_snapshots_for_one_destination_fail_closed_every_time() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let profile = temp.path();
        let first = profile.join(abandoned_residue_name("llama.dll", StagingRole::Restore, 8));
        let second = profile.join(abandoned_residue_name("llama.dll", StagingRole::Restore, 9));
        write(&first, b"one previous runtime");
        write(&second, b"another previous runtime");

        for _ in 0..2 {
            let error = recover_staging_residue(profile, &["llama.dll"])
                .expect_err("ambiguous ownership must fail closed");
            assert_eq!(error.kind(), io::ErrorKind::InvalidData);
            assert_eq!(read(&first), b"one previous runtime");
            assert_eq!(read(&second), b"another previous runtime");
            assert!(!profile.join("llama.dll").exists());
        }
    }

    #[test]
    fn recovery_rejects_a_destination_name_that_escapes_its_directory() {
        let temp = tempfile::tempdir().expect("temporary directory");
        for name in ["../llama.dll", "nested/llama.dll", "..", ""] {
            let error = recover_staging_residue(temp.path(), &[name])
                .expect_err("unsafe destination name must fail closed");
            assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        }
    }

    #[test]
    fn process_death_at_every_publication_transition_leaves_the_previous_runtime_usable() {
        for death_point in 0..=2 {
            let temp = tempfile::tempdir().expect("temporary directory");
            let profile = temp.path().join("debug");
            fs::create_dir_all(&profile).expect("profile directory");
            let first_source = temp.path().join("first-source.dll");
            let second_source = temp.path().join("second-source.dll");
            write(&first_source, b"new-first");
            write(&second_source, b"new-second");
            let first_destination = profile.join("first.dll");
            let second_destination = profile.join("second.dll");
            write(&first_destination, b"old-first");
            write(&second_destination, b"old-second");

            let prepared = vec![
                prepare_runtime_copy(&first_source, &first_destination)
                    .expect("prepare first copy"),
                prepare_runtime_copy(&second_source, &second_destination)
                    .expect("prepare second copy"),
            ];
            simulate_publication_death(prepared, death_point).expect("simulated process death");
            age_residue_into_a_dead_incarnation(&profile);

            let names = ["first.dll", "second.dll"];
            let recovered =
                recover_staging_residue(&profile, &names).expect("recovery after process death");
            assert!(
                !recovered.is_empty(),
                "death at transition {death_point} must leave owned recoverable residue"
            );
            assert_eq!(
                read(&first_destination),
                b"old-first",
                "death at transition {death_point} must leave the previous runtime usable"
            );
            assert_eq!(
                read(&second_destination),
                b"old-second",
                "death at transition {death_point} must leave the previous runtime usable"
            );
            assert_no_staging_residue(&profile);

            let repeated = recover_staging_residue(&profile, &names).expect("repeated recovery");
            assert!(repeated.is_empty(), "repeated recovery must be idempotent");
            assert_eq!(read(&first_destination), b"old-first");
            assert_eq!(read(&second_destination), b"old-second");
        }
    }

    #[test]
    fn staging_settles_abandoned_residue_before_it_publishes() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let profile = temp.path().join("debug");
        fs::create_dir_all(&profile).expect("profile directory");
        let source = temp.path().join("source.dll");
        write(&source, b"new-runtime");
        let destination = profile.join("llama.dll");
        let snapshot = profile.join(abandoned_residue_name(
            "llama.dll",
            StagingRole::Restore,
            10,
        ));
        let partial = profile.join(abandoned_residue_name("llama.dll", StagingRole::Copy, 11));
        write(&snapshot, b"previous-runtime");
        write(&partial, b"half-written");
        let foreign = profile.join("upstream-note.txt");
        write(&foreign, b"not ours");

        let recovered = stage_windows_runtime_files(&[(source.as_path(), destination.as_path())])
            .expect("stage the independent runtime copy");

        assert_eq!(recovered.len(), 2);
        assert!(
            recovered
                .iter()
                .any(|recovery| recovery.role == StagingRole::Restore
                    && recovery.decision == RecoveryDecision::RestoredPreviousRuntime)
        );
        assert!(
            recovered
                .iter()
                .any(|recovery| recovery.role == StagingRole::Copy
                    && recovery.decision == RecoveryDecision::Discarded)
        );
        assert_eq!(read(&destination), b"new-runtime");
        assert_eq!(read(&foreign), b"not ours");
        assert_no_staging_residue(&profile);
    }

    #[test]
    fn a_destination_swapped_after_its_snapshot_fails_before_replacement() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let profile = temp.path().join("debug");
        fs::create_dir_all(&profile).expect("profile directory");
        let first_source = temp.path().join("first-source.dll");
        let second_source = temp.path().join("second-source.dll");
        write(&first_source, b"new-first");
        write(&second_source, b"new-second");
        let first_destination = profile.join("first.dll");
        let second_destination = profile.join("second.dll");
        write(&first_destination, b"old-first");
        write(&second_destination, b"old-second");
        let first =
            prepare_runtime_copy(&first_source, &first_destination).expect("prepare first copy");
        let second =
            prepare_runtime_copy(&second_source, &second_destination).expect("prepare second copy");
        let swapped = second_destination.clone();
        let mut attempt = 0;

        let error = publish_all_with(vec![first, second], move |source, destination| {
            attempt += 1;
            replace_file(source, destination)?;
            if attempt == 1 {
                fs::remove_file(&swapped).expect("remove the swapped destination");
                fs::create_dir(&swapped).expect("swap the destination for a directory");
            }
            Ok(())
        })
        .expect_err("a swapped destination must fail before replacement");

        assert_eq!(error.kind(), io::ErrorKind::IsADirectory);
        assert_eq!(
            read(&first_destination),
            b"old-first",
            "the rolled back destination must hold the previous runtime"
        );
        assert!(second_destination.is_dir());
        let residue = entry_names(&profile)
            .into_iter()
            .filter(|name| name.contains(super::STAGING_RESIDUE_INFIX))
            .collect::<Vec<_>>();
        assert!(residue.is_empty(), "staging residue remains: {residue:?}");
    }

    #[cfg(unix)]
    #[test]
    fn a_destination_swapped_for_a_link_fails_before_replacement() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let profile = temp.path().join("debug");
        fs::create_dir_all(&profile).expect("profile directory");
        let source = temp.path().join("source.dll");
        write(&source, b"new-runtime");
        let destination = profile.join("llama.dll");
        write(&destination, b"previous-runtime");
        let outside = temp.path().join("outside.bin");
        write(&outside, b"outside the profile");
        let prepared = prepare_runtime_copy(&source, &destination).expect("prepare copy");
        // Renaming over a link silently consumes it, so only a re-check
        // immediately before the replacement can refuse the swap.
        fs::remove_file(&destination).expect("remove the validated destination");
        std::os::unix::fs::symlink(&outside, &destination).expect("swap in a reparse point");

        let error =
            super::publish_all(vec![prepared]).expect_err("a swapped destination must fail");

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(
            fs::read_link(&destination).expect("the swapped link survives"),
            outside
        );
        assert_eq!(read(&outside), b"outside the profile");
        assert_no_staging_residue(&profile);
    }

    #[test]
    fn recovery_ignores_a_missing_directory() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let missing: PathBuf = temp.path().join("never-created");
        assert!(
            recover_staging_residue(&missing, &["llama.dll"])
                .expect("absent directory is not an error")
                .is_empty()
        );
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::{prepare_runtime_copy, publish_all_with, replace_file, stage_windows_runtime_file};
    use std::fs;
    use std::io;

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

    #[test]
    fn late_publication_failure_restores_every_previous_dll() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let first_source = temp.path().join("first-source.dll");
        let second_source = temp.path().join("second-source.dll");
        let first_destination = temp.path().join("first.dll");
        let second_destination = temp.path().join("second.dll");
        fs::write(&first_source, b"new-first").expect("first source");
        fs::write(&second_source, b"new-second").expect("second source");
        fs::write(&first_destination, b"old-first").expect("first destination");
        fs::write(&second_destination, b"old-second").expect("second destination");
        let first =
            prepare_runtime_copy(&first_source, &first_destination).expect("first replacement");
        let second =
            prepare_runtime_copy(&second_source, &second_destination).expect("second replacement");
        let mut attempt = 0;

        publish_all_with(vec![first, second], |source, destination| {
            attempt += 1;
            if attempt == 2 {
                return Err(io::Error::other("injected late publication failure"));
            }
            replace_file(source, destination)
        })
        .expect_err("second publication must fail");

        assert_eq!(
            fs::read(first_destination).expect("restored first DLL"),
            b"old-first"
        );
        assert_eq!(
            fs::read(second_destination).expect("unchanged second DLL"),
            b"old-second"
        );
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
