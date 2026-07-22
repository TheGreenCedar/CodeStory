//! Native same-user transport and lifetime authority for the embedding server.
//!
//! Retrieval owns the protocol, scheduler, engine, and compatibility rules.
//! This module owns only the executable boundary and the platform primitives
//! that prove one local OS user is talking to one lifetime authority.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::time::Duration;

const INTERNAL_SERVER_COMMAND: &str = "internal-embedding-server";
const EXPECTED_EXECUTABLE_SHA256_ENV: &str =
    "CODESTORY_INTERNAL_EMBEDDING_SERVER_EXECUTABLE_SHA256";
const QUALIFICATION_DIR_ENV: &str = "CODESTORY_EMBED_QUALIFICATION_DIR";
const QUALIFICATION_NONCE_ENV: &str = "CODESTORY_EMBED_QUALIFICATION_NONCE";
const ENDPOINT_NAMESPACE: &str = "codestory-per-user-embedding-v1";
const CHILD_STDERR_TAIL_BYTES: usize = 8 * 1024;
const EXECUTABLE_ATTESTATION_SCHEMA_VERSION: u32 = 1;
type TransportIdentity = codestory_retrieval::EmbeddingTransportIdentity;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClientTransportMode {
    SpawnCapable,
    ObserveOnly,
}

trait ExecutableAttestationStore {
    fn endpoint_namespace_id(&self) -> &str;
    fn read(&self) -> Result<Option<Vec<u8>>>;
    fn publish(&self, content: &[u8]) -> Result<()>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExecutableDigestAttestation {
    schema_version: u32,
    endpoint_namespace_id: String,
    platform: String,
    architecture: String,
    executable_version: String,
    native_file_identity: String,
    file_size: u64,
    write_stamp: String,
    change_stamp: String,
    executable_sha256: String,
    record_sha256: String,
}

#[derive(Debug)]
pub(crate) enum NativeConnectOutcome {
    Connected(NativeEmbeddingStream),
    NoOwner,
    OwnerUnresponsive,
}

#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
pub(crate) enum NativeBindOutcome {
    Bound(NativeEmbeddingListener),
    AlreadyOwned,
}

#[derive(Debug, Clone)]
pub(crate) struct ExactExecutable {
    path: PathBuf,
    sha256: String,
    file_identity: ExecutableFileIdentity,
    version: &'static str,
}

impl ExactExecutable {
    pub(crate) fn capture() -> Result<Self> {
        let path = std::env::current_exe().context("resolve current CodeStory executable")?;
        let mut digest = sha256_reader;
        Self::capture_path(path, ClientTransportMode::SpawnCapable, None, &mut digest)
    }

    fn capture_for_client(mode: ClientTransportMode) -> Result<Self> {
        let path = std::env::current_exe().context("resolve current CodeStory executable")?;
        let store = matches!(mode, ClientTransportMode::ObserveOnly)
            .then(platform::executable_attestation_store)
            .and_then(Result::ok)
            .flatten();
        let mut digest = sha256_reader;
        Self::capture_path(
            path,
            mode,
            store.as_ref().map(|store| store as _),
            &mut digest,
        )
    }

    fn capture_path(
        path: PathBuf,
        mode: ClientTransportMode,
        store: Option<&dyn ExecutableAttestationStore>,
        digest: &mut dyn FnMut(&File, &Path) -> Result<String>,
    ) -> Result<Self> {
        let file = File::open(&path)
            .with_context(|| format!("open current executable {}", path.display()))?;
        let before = executable_file_identity(&file)
            .with_context(|| format!("inspect current executable {}", path.display()))?;
        let attested_sha256 = matches!(mode, ClientTransportMode::ObserveOnly)
            .then(|| store.and_then(|store| read_matching_attestation(store, &before)))
            .flatten();
        let sha256 = match attested_sha256 {
            Some(sha256) => sha256,
            None => digest(&file, &path)?,
        };
        let after = executable_file_identity(&file)
            .with_context(|| format!("reinspect current executable {}", path.display()))?;
        if before != after {
            bail!(
                "embedding_executable_changed: current executable changed while it was being verified"
            );
        }
        Ok(Self {
            path,
            sha256,
            file_identity: before,
            version: env!("CARGO_PKG_VERSION"),
        })
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn sha256(&self) -> &str {
        &self.sha256
    }

    pub(crate) fn version(&self) -> &'static str {
        self.version
    }

    fn verify_expected_server_digest(&self) -> Result<()> {
        let Some(expected) = std::env::var_os(EXPECTED_EXECUTABLE_SHA256_ENV) else {
            return Ok(());
        };
        let expected = expected.to_string_lossy();
        if !is_sha256(&expected) || !self.sha256.eq_ignore_ascii_case(&expected) {
            bail!(
                "embedding_executable_changed: spawned executable digest {} does not match expected {}",
                self.sha256,
                expected
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub(crate) struct NativeEmbeddingClientTransport {
    executable: ExactExecutable,
    executable_identity: codestory_retrieval::EmbeddingExecutableIdentity,
    mode: ClientTransportMode,
    clock: Arc<NativeAwakeClock>,
    next_spawn_generation: Arc<AtomicU64>,
}

impl NativeEmbeddingClientTransport {
    pub(crate) fn capture() -> Result<Self> {
        Self::capture_with_mode(ClientTransportMode::SpawnCapable)
    }

    pub(crate) fn capture_with_mode(mode: ClientTransportMode) -> Result<Self> {
        let executable = ExactExecutable::capture_for_client(mode)?;
        let executable_identity = executable_identity(&executable)?;
        Ok(Self {
            executable,
            executable_identity,
            mode,
            clock: Arc::new(NativeAwakeClock::capture()?),
            next_spawn_generation: Arc::new(AtomicU64::new(1)),
        })
    }

    pub(crate) fn connect(&self, budget: Duration) -> Result<NativeConnectOutcome> {
        self.connect_for_attempt(budget, None)
    }

    pub(crate) fn connect_for_attempt(
        &self,
        budget: Duration,
        spawn_attempt: Option<&codestory_retrieval::EmbeddingSpawnAttempt>,
    ) -> Result<NativeConnectOutcome> {
        let outcome = platform::connect(budget)?;
        if matches!(&outcome, NativeConnectOutcome::Connected(_)) {
            if let Some(spawn_attempt) = spawn_attempt {
                spawn_attempt.record_success();
            }
            return Ok(outcome);
        }
        if let Some(failure) =
            spawn_attempt.and_then(codestory_retrieval::EmbeddingSpawnAttempt::failure)
        {
            bail!("{failure}");
        }
        Ok(outcome)
    }

    pub(crate) fn spawn_exact_current_exe(
        &self,
    ) -> Result<codestory_retrieval::EmbeddingSpawnAttempt> {
        if self.mode == ClientTransportMode::ObserveOnly {
            bail!("embedding_server_spawn_forbidden: observe-only transport cannot spawn");
        }
        // Keep the verified candidate open until spawn so replacement cannot
        // silently turn metadata validation into validation of one file and
        // execution of another on platforms that deny replacement of open
        // images. Unix ctime is part of the captured identity, so an ordinary
        // owner cannot restore the key after rewriting the inode. Windows
        // metadata has no exposed change time and therefore still requires a
        // fresh digest.
        let candidate = File::open(self.executable.path()).with_context(|| {
            format!(
                "open captured executable {}",
                self.executable.path().display()
            )
        })?;
        let before = executable_file_identity(&candidate)?;
        #[cfg(windows)]
        let digest = sha256_reader(&candidate, self.executable.path())?;
        let after = executable_file_identity(&candidate)?;
        let digest_matches = {
            #[cfg(unix)]
            {
                true
            }
            #[cfg(windows)]
            {
                digest.eq_ignore_ascii_case(self.executable.sha256())
            }
        };
        if before != self.executable.file_identity || before != after || !digest_matches {
            bail!(
                "embedding_executable_changed: current executable no longer matches the captured exact executable"
            );
        }

        if let Ok(Some(store)) = platform::executable_attestation_store() {
            let _ = publish_attestation(&store, &self.executable);
        }

        let generation = self
            .next_spawn_generation
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                current.checked_add(1)
            })
            .map_err(|_| anyhow::anyhow!("embedding_server_spawn_generation_exhausted"))?;
        let spawn_attempt = codestory_retrieval::EmbeddingSpawnAttempt::new(generation);
        let mut command = Command::new(self.executable.path());
        command
            .arg(INTERNAL_SERVER_COMMAND)
            .env(EXPECTED_EXECUTABLE_SHA256_ENV, self.executable.sha256())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        platform::detach_command(&mut command);
        let mut child = command.spawn().with_context(|| {
            format!(
                "spawn exact executable {}",
                self.executable.path().display()
            )
        })?;
        let mut child_stderr = child
            .stderr
            .take()
            .context("capture embedding server startup diagnostics")?;
        let reaper_attempt = spawn_attempt.clone();
        std::thread::Builder::new()
            .name("codestory-embedding-server-reaper".into())
            .spawn(move || {
                let mut tail = Vec::new();
                let mut buffer = [0_u8; 1024];
                loop {
                    match child_stderr.read(&mut buffer) {
                        Ok(0) => break,
                        Ok(read) => {
                            tail.extend_from_slice(&buffer[..read]);
                            if tail.len() > CHILD_STDERR_TAIL_BYTES {
                                tail.drain(..tail.len() - CHILD_STDERR_TAIL_BYTES);
                            }
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                        Err(_) => break,
                    }
                }
                if let Ok(status) = child.wait() {
                    if status.success() {
                        reaper_attempt.record_success();
                    } else {
                        let diagnostics = String::from_utf8_lossy(&tail)
                            .split_whitespace()
                            .collect::<Vec<_>>()
                            .join(" ");
                        let diagnostics = if diagnostics.is_empty() {
                            "no child diagnostics".to_string()
                        } else {
                            diagnostics
                        };
                        reaper_attempt.record_failure(
                            codestory_retrieval::EmbeddingTransportFailure {
                                code: "embedding_server_start_failed".into(),
                                message: format!(
                                    "internal server exited with {status}; {diagnostics}"
                                ),
                            },
                        );
                    }
                }
            })
            .context("start embedding server child reaper")?;
        Ok(spawn_attempt)
    }
}

#[derive(Debug)]
pub(crate) struct NativeEmbeddingListener {
    inner: platform::Listener,
    closed: AtomicBool,
}

impl NativeEmbeddingListener {
    pub(crate) fn bind() -> Result<NativeBindOutcome> {
        platform::bind().map(|outcome| match outcome {
            platform::BindOutcome::Bound(inner) => {
                NativeBindOutcome::Bound(NativeEmbeddingListener {
                    inner,
                    closed: AtomicBool::new(false),
                })
            }
            platform::BindOutcome::AlreadyOwned => NativeBindOutcome::AlreadyOwned,
        })
    }

    pub(crate) fn identity(&self) -> &TransportIdentity {
        self.inner.identity()
    }

    pub(crate) fn accept(&self, timeout: Duration) -> Result<Option<NativeEmbeddingStream>> {
        if self.closed.load(Ordering::Acquire) {
            return Ok(None);
        }
        let started = platform::awake_now_ns();
        loop {
            let elapsed = Duration::from_nanos(platform::awake_now_ns().saturating_sub(started));
            let remaining = timeout.saturating_sub(elapsed);
            if remaining.is_zero() {
                return Ok(None);
            }
            match self.inner.accept(remaining) {
                Ok(stream) => {
                    return Ok(stream.map(|inner| NativeEmbeddingStream { inner }));
                }
                Err(error) if rejected_peer_error(&error) => {}
                Err(error) => return Err(error),
            }
        }
    }

    fn close(&self) {
        self.closed.store(true, Ordering::Release);
    }
}

fn rejected_peer_error(error: &anyhow::Error) -> bool {
    error
        .chain()
        .any(|cause| cause.to_string() == "embedding_peer_rejected")
}

#[derive(Debug)]
pub(crate) struct NativeEmbeddingStream {
    inner: platform::Stream,
}

impl NativeEmbeddingStream {
    pub(crate) fn identity(&self) -> &TransportIdentity {
        self.inner.identity()
    }

    fn set_read_timeout(&self, timeout: Option<Duration>) -> std::io::Result<()> {
        self.inner.set_read_timeout(timeout)
    }

    fn set_write_timeout(&self, timeout: Option<Duration>) -> std::io::Result<()> {
        self.inner.set_write_timeout(timeout)
    }

    fn shutdown(&self) -> std::io::Result<()> {
        self.inner.shutdown()
    }

    fn finish_response_delivery(&self) -> std::io::Result<()> {
        self.inner.finish_response_delivery()
    }

    fn peer_is_alive(&self) -> std::io::Result<bool> {
        self.inner.peer_is_alive()
    }

    fn peer_exit_code(&self) -> std::io::Result<Option<u32>> {
        #[cfg(windows)]
        {
            return self.inner.peer_exit_code();
        }
        #[cfg(unix)]
        {
            Ok(None)
        }
    }
}

impl Read for NativeEmbeddingStream {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.inner.read(buffer)
    }
}

impl Write for NativeEmbeddingStream {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.inner.write(buffer)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

pub(crate) fn run_internal_embedding_server() -> Result<()> {
    let executable = ExactExecutable::capture()?;
    executable.verify_expected_server_digest()?;
    let listener = match NativeEmbeddingListener::bind()? {
        NativeBindOutcome::Bound(listener) => listener,
        NativeBindOutcome::AlreadyOwned => return Ok(()),
    };
    run_retrieval_server(listener, executable)
}

fn run_retrieval_server(
    listener: NativeEmbeddingListener,
    executable: ExactExecutable,
) -> Result<()> {
    std::hint::black_box(codestory_retrieval::PER_USER_EMBEDDING_SERVER_PROOF_MARKER);
    let defaults = crate::sidecar_runtime::process_defaults();
    let transport = Arc::new(NativeEmbeddingServerTransport {
        listener: std::sync::Mutex::new(Some(listener)),
        clock: Arc::new(NativeAwakeClock::capture()?),
    });
    let executable = executable_identity(&executable)?;
    codestory_retrieval::run_per_user_embedding_server(
        codestory_retrieval::PerUserEmbeddingServerConfig {
            transport,
            engine_cache_root: defaults.cache_root().to_path_buf(),
            executable,
            allow_cpu: defaults.embedding_allow_cpu(),
            budgets: codestory_retrieval::EmbeddingServerBudgets::current(),
            protocol: codestory_retrieval::EmbeddingServerProtocolSnapshot::current(),
        },
    )
}

pub(crate) fn install_client_transport(mode: ClientTransportMode) -> Result<()> {
    codestory_retrieval::install_embedding_client_transport(Arc::new(
        NativeEmbeddingClientTransport::capture_with_mode(mode)?,
    ))
}

pub(crate) fn clock_domain() -> &'static str {
    "awake_monotonic"
}

pub(crate) fn clock_api() -> &'static str {
    platform::clock_api()
}

pub(crate) fn boot_id() -> Result<String> {
    platform::boot_id()
}

pub(crate) fn inclusive_clock_api() -> &'static str {
    platform::inclusive_clock_api()
}

pub(crate) fn inclusive_now_ns() -> Result<u64> {
    platform::inclusive_now_ns()
}

#[derive(Debug)]
struct NativeAwakeClock {
    snapshot: codestory_retrieval::EmbeddingServerClockSnapshot,
}

impl NativeAwakeClock {
    fn capture() -> Result<Self> {
        Ok(Self {
            snapshot: codestory_retrieval::EmbeddingServerClockSnapshot {
                domain: clock_domain().into(),
                api: clock_api().into(),
                boot_id: boot_id()?,
                resolution_ns: platform::clock_resolution_ns()?,
            },
        })
    }
}

impl codestory_retrieval::AwakeMonotonicClock for NativeAwakeClock {
    fn now_ns(&self) -> u64 {
        platform::awake_now_ns()
    }

    fn sleep(&self, duration: Duration) {
        std::thread::sleep(duration);
    }

    fn snapshot(&self) -> codestory_retrieval::EmbeddingServerClockSnapshot {
        self.snapshot.clone()
    }
}

impl codestory_retrieval::EmbeddingServerStream for NativeEmbeddingStream {
    fn transport_identity(&self) -> &codestory_retrieval::EmbeddingTransportIdentity {
        self.identity()
    }

    fn set_read_timeout(&self, timeout: Option<Duration>) -> std::io::Result<()> {
        NativeEmbeddingStream::set_read_timeout(self, timeout)
    }

    fn set_write_timeout(&self, timeout: Option<Duration>) -> std::io::Result<()> {
        NativeEmbeddingStream::set_write_timeout(self, timeout)
    }

    fn shutdown(&self) -> std::io::Result<()> {
        NativeEmbeddingStream::shutdown(self)
    }

    fn finish_response_delivery(&self) -> std::io::Result<()> {
        NativeEmbeddingStream::finish_response_delivery(self)
    }

    fn peer_is_alive(&self) -> std::io::Result<bool> {
        NativeEmbeddingStream::peer_is_alive(self)
    }

    fn peer_exit_code(&self) -> std::io::Result<Option<u32>> {
        NativeEmbeddingStream::peer_exit_code(self)
    }
}

impl codestory_retrieval::EmbeddingClientTransport for NativeEmbeddingClientTransport {
    fn connect(
        &self,
        _intent: codestory_retrieval::EmbeddingConnectIntent,
        budget: Duration,
        spawn_attempt: Option<&codestory_retrieval::EmbeddingSpawnAttempt>,
    ) -> std::result::Result<
        codestory_retrieval::EmbeddingConnectOutcome,
        codestory_retrieval::EmbeddingTransportFailure,
    > {
        self.connect_for_attempt(budget, spawn_attempt)
            .map(|outcome| match outcome {
                NativeConnectOutcome::Connected(stream) => {
                    codestory_retrieval::EmbeddingConnectOutcome::Connected(Box::new(stream))
                }
                NativeConnectOutcome::NoOwner => {
                    codestory_retrieval::EmbeddingConnectOutcome::NoOwner
                }
                NativeConnectOutcome::OwnerUnresponsive => {
                    codestory_retrieval::EmbeddingConnectOutcome::OwnerUnresponsive(
                        codestory_retrieval::EmbeddingTransportFailure {
                            code: "embedding_server_owner_unresponsive".into(),
                            message:
                                "the per-user lifetime authority exists but did not accept a connection"
                                    .into(),
                        },
                    )
                }
            })
            .map_err(|error| transport_failure("embedding_transport_connect_failed", error))
    }

    fn spawn_exact_current_exe(
        &self,
    ) -> std::result::Result<
        codestory_retrieval::EmbeddingSpawnAttempt,
        codestory_retrieval::EmbeddingTransportFailure,
    > {
        self.spawn_exact_current_exe()
            .map_err(|error| transport_failure("embedding_server_spawn_failed", error))
    }

    fn clock(&self) -> Arc<dyn codestory_retrieval::AwakeMonotonicClock> {
        self.clock.clone()
    }

    fn executable_identity(&self) -> codestory_retrieval::EmbeddingExecutableIdentity {
        self.executable_identity.clone()
    }

    fn budgets(&self) -> codestory_retrieval::EmbeddingClientBudgets {
        codestory_retrieval::EmbeddingClientBudgets::current()
    }
}

struct NativeEmbeddingServerTransport {
    listener: std::sync::Mutex<Option<NativeEmbeddingListener>>,
    clock: Arc<NativeAwakeClock>,
}

impl codestory_retrieval::EmbeddingServerTransport for NativeEmbeddingServerTransport {
    fn bind(
        &self,
    ) -> std::result::Result<
        codestory_retrieval::EmbeddingServerBindOutcome,
        codestory_retrieval::EmbeddingTransportFailure,
    > {
        let listener = self
            .listener
            .lock()
            .map_err(|_| codestory_retrieval::EmbeddingTransportFailure {
                code: "embedding_transport_state_poisoned".into(),
                message: "embedding server transport state was poisoned".into(),
            })?
            .take();
        match listener {
            Some(listener) => Ok(codestory_retrieval::EmbeddingServerBindOutcome::Bound(
                Box::new(listener),
            )),
            None => Ok(codestory_retrieval::EmbeddingServerBindOutcome::AlreadyOwned),
        }
    }

    fn clock(&self) -> Arc<dyn codestory_retrieval::AwakeMonotonicClock> {
        self.clock.clone()
    }

    fn fail_stop(&self, reason_code: &str) {
        fail_stop_process(reason_code);
    }
}

fn fail_stop_process(_reason_code: &str) -> ! {
    // A spawned server can outlive the process that owns its stderr reader.
    // Fail-stop must not perform fallible I/O before terminating the process.
    std::process::abort()
}

impl codestory_retrieval::EmbeddingServerListener for NativeEmbeddingListener {
    fn accept(
        &self,
        timeout: Duration,
    ) -> std::result::Result<
        Option<Box<dyn codestory_retrieval::EmbeddingServerStream>>,
        codestory_retrieval::EmbeddingTransportFailure,
    > {
        NativeEmbeddingListener::accept(self, timeout)
            .map(|stream| {
                stream.map(|stream| {
                    Box::new(stream) as Box<dyn codestory_retrieval::EmbeddingServerStream>
                })
            })
            .map_err(|error| transport_failure("embedding_transport_accept_failed", error))
    }

    fn identity(&self) -> &codestory_retrieval::EmbeddingTransportIdentity {
        NativeEmbeddingListener::identity(self)
    }

    fn close(&self) -> std::result::Result<(), codestory_retrieval::EmbeddingTransportFailure> {
        NativeEmbeddingListener::close(self);
        Ok(())
    }
}

fn executable_identity(
    executable: &ExactExecutable,
) -> Result<codestory_retrieval::EmbeddingExecutableIdentity> {
    let process_start_id =
        match codestory_retrieval::probe_process_start_identity(std::process::id()) {
            codestory_retrieval::ProcessStartProbe::Running { start_identity } => start_identity,
            codestory_retrieval::ProcessStartProbe::NotRunning => {
                bail!("current CodeStory process is not running")
            }
            codestory_retrieval::ProcessStartProbe::Unknown { reason } => {
                bail!("current CodeStory process start identity is unavailable: {reason}")
            }
        };
    Ok(codestory_retrieval::EmbeddingExecutableIdentity {
        pid: std::process::id(),
        process_start_id,
        executable_sha256: executable.sha256().into(),
        executable_version: executable.version().into(),
    })
}

fn read_matching_attestation(
    store: &dyn ExecutableAttestationStore,
    identity: &ExecutableFileIdentity,
) -> Option<String> {
    let content = store.read().ok().flatten()?;
    let attestation = serde_json::from_slice::<ExecutableDigestAttestation>(&content).ok()?;
    let expected = executable_attestation(
        store.endpoint_namespace_id(),
        identity,
        &attestation.executable_sha256,
    );
    (attestation == expected && is_sha256(&attestation.executable_sha256))
        .then_some(attestation.executable_sha256)
}

fn publish_attestation(
    store: &dyn ExecutableAttestationStore,
    executable: &ExactExecutable,
) -> Result<()> {
    let attestation = executable_attestation(
        store.endpoint_namespace_id(),
        &executable.file_identity,
        executable.sha256(),
    );
    let content =
        serde_json::to_vec(&attestation).context("encode executable digest attestation")?;
    store.publish(&content)
}

fn executable_attestation(
    endpoint_namespace_id: &str,
    identity: &ExecutableFileIdentity,
    executable_sha256: &str,
) -> ExecutableDigestAttestation {
    let mut attestation = ExecutableDigestAttestation {
        schema_version: EXECUTABLE_ATTESTATION_SCHEMA_VERSION,
        endpoint_namespace_id: endpoint_namespace_id.into(),
        platform: std::env::consts::OS.into(),
        architecture: std::env::consts::ARCH.into(),
        executable_version: env!("CARGO_PKG_VERSION").into(),
        native_file_identity: identity.attestation_native_file_identity(),
        file_size: identity.file_size,
        write_stamp: identity.write_stamp.to_string(),
        change_stamp: identity.change_stamp.to_string(),
        executable_sha256: executable_sha256.into(),
        record_sha256: String::new(),
    };
    attestation.record_sha256 = executable_attestation_record_sha256(&attestation);
    attestation
}

fn executable_attestation_record_sha256(attestation: &ExecutableDigestAttestation) -> String {
    sha256_fields(&[
        b"codestory-executable-digest-attestation-record-v1",
        attestation.schema_version.to_string().as_bytes(),
        attestation.endpoint_namespace_id.as_bytes(),
        attestation.platform.as_bytes(),
        attestation.architecture.as_bytes(),
        attestation.executable_version.as_bytes(),
        attestation.native_file_identity.as_bytes(),
        attestation.file_size.to_string().as_bytes(),
        attestation.write_stamp.as_bytes(),
        attestation.change_stamp.as_bytes(),
        attestation.executable_sha256.as_bytes(),
    ])
}

fn transport_failure(
    fallback_code: &str,
    error: anyhow::Error,
) -> codestory_retrieval::EmbeddingTransportFailure {
    let message = format!("{error:#}");
    let code = message
        .split_once(':')
        .map(|(code, _)| code)
        .filter(|code| code.starts_with("embedding_"))
        .unwrap_or(fallback_code);
    codestory_retrieval::EmbeddingTransportFailure {
        code: code.into(),
        message,
    }
}

fn sha256_reader(file: &File, path: &Path) -> Result<String> {
    let mut reader = file;
    let mut hasher = Sha256::new();
    std::io::copy(&mut reader, &mut hasher).with_context(|| format!("hash {}", path.display()))?;
    Ok(format!("{:x}", hasher.finalize()))
}

fn sha256_fields(fields: &[&[u8]]) -> String {
    let mut hasher = Sha256::new();
    for field in fields {
        hasher.update((field.len() as u64).to_le_bytes());
        hasher.update(field);
    }
    format!("{:x}", hasher.finalize())
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn remaining_awake_budget(started_ns: u64, now_ns: u64, budget: Duration) -> Option<Duration> {
    let elapsed = Duration::from_nanos(now_ns.saturating_sub(started_ns));
    (elapsed < budget).then(|| budget.saturating_sub(elapsed))
}

#[cfg(windows)]
fn awake_deadline_ns(started_ns: u64, budget: Duration) -> u64 {
    started_ns.saturating_add(budget.as_nanos().min(u128::from(u64::MAX)) as u64)
}

#[cfg(any(windows, test))]
const WINDOWS_ERROR_FILE_NOT_FOUND_CODE: u32 = 2;

#[cfg(any(windows, test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RetainedWindowsAuthorityState {
    Absent,
    Live,
}

#[cfg(any(windows, test))]
fn classify_windows_data_pipe_open_error(
    error_code: u32,
    authority: RetainedWindowsAuthorityState,
) -> Option<NativeConnectOutcome> {
    (error_code == WINDOWS_ERROR_FILE_NOT_FOUND_CODE).then(|| match authority {
        RetainedWindowsAuthorityState::Absent => NativeConnectOutcome::NoOwner,
        RetainedWindowsAuthorityState::Live => NativeConnectOutcome::OwnerUnresponsive,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ExecutableFileIdentity {
    native_identity: codestory_workspace::WorkspacePathIdentity,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    file_size: u64,
    write_stamp: i128,
    change_stamp: i128,
}

impl ExecutableFileIdentity {
    #[cfg(unix)]
    fn attestation_native_file_identity(&self) -> String {
        sha256_fields(&[
            b"codestory-unix-executable-file-identity-v1",
            self.device.to_string().as_bytes(),
            self.inode.to_string().as_bytes(),
        ])
    }

    #[cfg(windows)]
    fn attestation_native_file_identity(&self) -> String {
        // Windows does not consume cached executable attestations. Keep the
        // serialized shape complete for exact-capture diagnostics without
        // pretending last-write metadata is a strong change token.
        "windows-fresh-hash-required".into()
    }
}

#[cfg(unix)]
fn executable_file_identity(file: &File) -> Result<ExecutableFileIdentity> {
    use std::os::unix::fs::MetadataExt;

    let metadata = file.metadata()?;
    if !metadata.is_file() {
        bail!("expected a regular executable");
    }
    Ok(ExecutableFileIdentity {
        native_identity: codestory_workspace::workspace_file_identity(file)?,
        device: metadata.dev(),
        inode: metadata.ino(),
        file_size: metadata.len(),
        write_stamp: (i128::from(metadata.mtime()) << 64)
            | i128::from(metadata.mtime_nsec() as u64),
        change_stamp: (i128::from(metadata.ctime()) << 64)
            | i128::from(metadata.ctime_nsec() as u64),
    })
}

#[cfg(windows)]
fn executable_file_identity(file: &File) -> Result<ExecutableFileIdentity> {
    use std::os::windows::fs::MetadataExt;

    let metadata = file.metadata()?;
    if !metadata.is_file() {
        bail!("expected a regular executable");
    }
    Ok(ExecutableFileIdentity {
        native_identity: codestory_workspace::workspace_file_identity(file)?,
        file_size: metadata.file_size(),
        write_stamp: i128::from(metadata.last_write_time()),
        change_stamp: 0,
    })
}

#[cfg(unix)]
mod platform {
    use super::{
        ENDPOINT_NAMESPACE, ExecutableAttestationStore, NativeConnectOutcome,
        QUALIFICATION_DIR_ENV, QUALIFICATION_NONCE_ENV, TransportIdentity, sha256_fields,
    };
    use anyhow::{Context, Result, bail};
    use std::ffi::CString as UnixCString;
    #[cfg(target_os = "macos")]
    use std::ffi::{CStr, CString, OsStr};
    use std::fs::{self, DirBuilder, File, OpenOptions};
    use std::io::{ErrorKind, Read, Write};
    use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd, RawFd};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::{
        DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt,
    };
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    const SERVER_DIR_NAME: &str = "codestory-embedding-v1";
    const QUALIFICATION_SERVER_DIR_NAME: &str = "cs-eq1";
    const SOCKET_NAME: &str = "server.sock";
    const LOCK_NAME: &str = "codestory-embedding-v1.authority.lock";
    const PRIVATE_DIR_MODE: u32 = 0o700;
    const PRIVATE_FILE_MODE: u32 = 0o600;
    const EXECUTABLE_ATTESTATION_MAX_BYTES: u64 = 4 * 1024;
    const ACCEPT_POLL: Duration = Duration::from_millis(10);
    const STREAM_IO_POLL: Duration = Duration::from_millis(25);
    const NO_TIMEOUT: u64 = u64::MAX;

    #[derive(Debug)]
    #[allow(clippy::large_enum_variant)]
    pub(super) enum BindOutcome {
        Bound(Listener),
        AlreadyOwned,
    }

    #[derive(Debug)]
    struct RuntimeDirectory {
        path: PathBuf,
        handle: File,
        dev: u64,
        ino: u64,
        uid: u32,
        forbidden_mode_bits: u32,
        endpoint_namespace_id: String,
    }

    pub(super) struct NativeExecutableAttestationStore {
        authority: RuntimeDirectory,
        endpoint_namespace_id: String,
        file_name: String,
    }

    impl ExecutableAttestationStore for NativeExecutableAttestationStore {
        fn endpoint_namespace_id(&self) -> &str {
            &self.endpoint_namespace_id
        }

        fn read(&self) -> Result<Option<Vec<u8>>> {
            ensure_runtime_directory_matches(&self.authority)?;
            let name = UnixCString::new(self.file_name.as_str())
                .context("executable attestation name contains NUL")?;
            let fd = unsafe {
                libc::openat(
                    self.authority.handle.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                )
            };
            if fd < 0 {
                let error = std::io::Error::last_os_error();
                if error.kind() == ErrorKind::NotFound {
                    return Ok(None);
                }
                return Err(error).context("open executable digest attestation");
            }
            let mut file = unsafe { File::from_raw_fd(fd) };
            validate_attestation_file(&file)?;
            let mut content = Vec::new();
            std::io::Read::by_ref(&mut file)
                .take(EXECUTABLE_ATTESTATION_MAX_BYTES + 1)
                .read_to_end(&mut content)
                .context("read executable digest attestation")?;
            if content.len() as u64 > EXECUTABLE_ATTESTATION_MAX_BYTES {
                bail!("embedding_executable_attestation_untrusted: file exceeds size limit");
            }
            validate_attestation_file(&file)?;
            ensure_runtime_directory_matches(&self.authority)?;
            Ok(Some(content))
        }

        fn publish(&self, content: &[u8]) -> Result<()> {
            if content.len() as u64 > EXECUTABLE_ATTESTATION_MAX_BYTES {
                bail!("embedding_executable_attestation_too_large");
            }
            ensure_runtime_directory_matches(&self.authority)?;
            static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
            let sequence = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
            let temp_name = format!(
                ".{}.{}.{}.tmp",
                self.file_name,
                std::process::id(),
                sequence
            );
            let temp = UnixCString::new(temp_name.as_str())
                .context("temporary executable attestation name contains NUL")?;
            let destination = UnixCString::new(self.file_name.as_str())
                .context("executable attestation name contains NUL")?;
            let fd = unsafe {
                libc::openat(
                    self.authority.handle.as_raw_fd(),
                    temp.as_ptr(),
                    libc::O_WRONLY
                        | libc::O_CREAT
                        | libc::O_EXCL
                        | libc::O_CLOEXEC
                        | libc::O_NOFOLLOW,
                    PRIVATE_FILE_MODE,
                )
            };
            if fd < 0 {
                return Err(std::io::Error::last_os_error())
                    .context("create temporary executable digest attestation");
            }
            let mut file = unsafe { File::from_raw_fd(fd) };
            let result = (|| {
                file.write_all(content)
                    .context("write executable digest attestation")?;
                file.sync_all()
                    .context("sync executable digest attestation")?;
                validate_attestation_file(&file)?;
                ensure_runtime_directory_matches(&self.authority)?;
                if unsafe {
                    libc::renameat(
                        self.authority.handle.as_raw_fd(),
                        temp.as_ptr(),
                        self.authority.handle.as_raw_fd(),
                        destination.as_ptr(),
                    )
                } != 0
                {
                    return Err(std::io::Error::last_os_error())
                        .context("publish executable digest attestation");
                }
                self.authority
                    .handle
                    .sync_all()
                    .context("sync executable attestation authority")?;
                ensure_runtime_directory_matches(&self.authority)
            })();
            if result.is_err() {
                let _ =
                    unsafe { libc::unlinkat(self.authority.handle.as_raw_fd(), temp.as_ptr(), 0) };
            }
            result
        }
    }

    fn validate_attestation_file(file: &File) -> Result<()> {
        let metadata = file
            .metadata()
            .context("inspect executable digest attestation")?;
        if !metadata.is_file()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.nlink() != 1
            || metadata.mode() & 0o077 != 0
            || metadata.len() > EXECUTABLE_ATTESTATION_MAX_BYTES
        {
            bail!("embedding_executable_attestation_untrusted");
        }
        Ok(())
    }

    #[derive(Debug)]
    struct RuntimePaths {
        socket_base: PathBuf,
        authority_directory: PathBuf,
        namespace_salt: String,
        server_dir_name: String,
        socket_name: String,
        expected_authority_identity: Option<(u64, u64, u32)>,
        authority_is_private: bool,
        authority_has_fixed_parent: bool,
    }

    #[derive(Debug)]
    pub(super) struct Listener {
        listener: UnixListener,
        runtime: RuntimeDirectory,
        authority_directory: RuntimeDirectory,
        authority: File,
        socket_name: String,
        socket_identity: (u64, u64),
        identity: TransportIdentity,
    }

    impl Listener {
        pub(super) fn identity(&self) -> &TransportIdentity {
            &self.identity
        }

        pub(super) fn accept(&self, timeout: Duration) -> Result<Option<Stream>> {
            let started = awake_now_ns();
            loop {
                match self.listener.accept() {
                    Ok((stream, _)) => {
                        return authenticate_peer(stream, self.runtime.uid, self.identity.clone())
                            .map(Some)
                            .context("embedding_peer_rejected");
                    }
                    Err(error) if error.kind() == ErrorKind::WouldBlock => {
                        let elapsed = Duration::from_nanos(awake_now_ns().saturating_sub(started));
                        if elapsed >= timeout {
                            return Ok(None);
                        }
                        std::thread::sleep(ACCEPT_POLL.min(timeout.saturating_sub(elapsed)));
                    }
                    Err(error) => return Err(error).context("accept embedding server connection"),
                }
            }
        }
    }

    fn authenticate_peer(
        stream: UnixStream,
        expected_uid: u32,
        identity: TransportIdentity,
    ) -> Result<Stream> {
        let peer = peer_identity(stream.as_raw_fd())?;
        if peer.uid != expected_uid {
            bail!(
                "embedding_peer_identity_mismatch: accepted uid {} but expected {}",
                peer.uid,
                expected_uid
            );
        }
        let peer_pid = peer
            .pid
            .context("authenticated Unix peer PID is unavailable")?;
        let peer_process_start_id = canonical_peer_process_start_identity(peer_pid)?;
        Stream::new(
            stream,
            TransportIdentity {
                peer_pid: peer.pid,
                peer_process_start_id: Some(peer_process_start_id),
                ..identity
            },
        )
    }

    impl Drop for Listener {
        fn drop(&mut self) {
            // Hold the lifetime authority until the exact listener entry has
            // been removed. Releasing it first lets a replacement bind and
            // then lose its fresh socket to this old listener's destructor.
            if runtime_directory_matches(&self.authority_directory)
                && runtime_directory_matches(&self.runtime)
                && socket_identity_at(&self.runtime, &self.socket_name)
                    .ok()
                    .flatten()
                    == Some(self.socket_identity)
            {
                let _ = unlink_socket_at(&self.runtime, &self.socket_name);
            }
            let _ = unlock(self.authority.as_raw_fd());
        }
    }

    #[derive(Debug)]
    pub(super) struct Stream {
        stream: UnixStream,
        identity: TransportIdentity,
        peer_start_identity: String,
        read_timeout_ns: AtomicU64,
        write_timeout_ns: AtomicU64,
    }

    impl Stream {
        fn new(stream: UnixStream, identity: TransportIdentity) -> Result<Self> {
            let peer_pid = identity
                .peer_pid
                .context("authenticated Unix peer PID is unavailable")?;
            let peer_start_identity = peer_process_start_identity(peer_pid)?
                .context("authenticated Unix peer exited before identity capture")?;
            let canonical_start_identity = canonical_peer_process_start_identity(peer_pid)?;
            if identity.peer_process_start_id.as_deref() != Some(&canonical_start_identity) {
                bail!("embedding_peer_process_identity_changed");
            }
            stream
                .set_nonblocking(true)
                .context("set embedding Unix stream nonblocking")?;
            Ok(Self {
                stream,
                identity,
                peer_start_identity,
                read_timeout_ns: AtomicU64::new(NO_TIMEOUT),
                write_timeout_ns: AtomicU64::new(NO_TIMEOUT),
            })
        }

        pub(super) fn identity(&self) -> &TransportIdentity {
            &self.identity
        }

        pub(super) fn set_read_timeout(&self, timeout: Option<Duration>) -> std::io::Result<()> {
            self.read_timeout_ns
                .store(timeout_ns(timeout), Ordering::Release);
            Ok(())
        }

        pub(super) fn set_write_timeout(&self, timeout: Option<Duration>) -> std::io::Result<()> {
            self.write_timeout_ns
                .store(timeout_ns(timeout), Ordering::Release);
            Ok(())
        }

        pub(super) fn shutdown(&self) -> std::io::Result<()> {
            self.stream.shutdown(std::net::Shutdown::Both)
        }

        pub(super) fn finish_response_delivery(&self) -> std::io::Result<()> {
            Ok(())
        }

        pub(super) fn peer_is_alive(&self) -> std::io::Result<bool> {
            let pid = self.identity.peer_pid.ok_or_else(|| {
                std::io::Error::new(
                    ErrorKind::InvalidData,
                    "authenticated Unix peer PID is unavailable",
                )
            })?;
            Ok(peer_process_start_identity(pid)?
                .is_some_and(|identity| identity == self.peer_start_identity))
        }
    }

    impl Read for Stream {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            if buffer.is_empty() {
                return Ok(0);
            }
            let started = awake_now_ns();
            loop {
                match self.stream.read(buffer) {
                    Ok(read) => return Ok(read),
                    Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                    Err(error) if error.kind() == ErrorKind::WouldBlock => {
                        poll_stream(
                            self.stream.as_raw_fd(),
                            libc::POLLIN,
                            started,
                            self.read_timeout_ns.load(Ordering::Acquire),
                            "read",
                        )?;
                    }
                    Err(error) => return Err(error),
                }
            }
        }
    }

    impl Write for Stream {
        fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
            if buffer.is_empty() {
                return Ok(0);
            }
            let started = awake_now_ns();
            loop {
                match self.stream.write(buffer) {
                    Ok(written) => return Ok(written),
                    Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                    Err(error) if error.kind() == ErrorKind::WouldBlock => {
                        poll_stream(
                            self.stream.as_raw_fd(),
                            libc::POLLOUT,
                            started,
                            self.write_timeout_ns.load(Ordering::Acquire),
                            "write",
                        )?;
                    }
                    Err(error) => return Err(error),
                }
            }
        }

        fn flush(&mut self) -> std::io::Result<()> {
            self.stream.flush()
        }
    }

    pub(super) fn connect(budget: Duration) -> Result<NativeConnectOutcome> {
        let started = awake_now_ns();
        let paths = runtime_paths()?;
        let authority_directory = authority_directory(&paths)?;
        let authority = open_authority(&authority_directory, false)?;
        let runtime = runtime_directory(&paths, &authority_directory, false)?;
        let Some(runtime) = runtime else {
            return classify_failed_connect(authority);
        };
        let socket_path = runtime.path.join(&paths.socket_name);
        match validate_socket_path(&socket_path, runtime.uid) {
            Ok(Some(listener_id)) => match bounded_connect(
                &socket_path,
                match super::remaining_awake_budget(started, awake_now_ns(), budget) {
                    Some(remaining) => remaining,
                    None => return classify_failed_connect(authority),
                },
            ) {
                Ok(stream) => {
                    let peer = peer_identity(stream.as_raw_fd())?;
                    if peer.uid != runtime.uid {
                        bail!(
                            "embedding_peer_identity_mismatch: connected uid {} but expected {}",
                            peer.uid,
                            runtime.uid
                        );
                    }
                    let peer_pid = peer
                        .pid
                        .context("authenticated Unix peer PID is unavailable")?;
                    let peer_process_start_id = canonical_peer_process_start_identity(peer_pid)?;
                    if super::remaining_awake_budget(started, awake_now_ns(), budget).is_none() {
                        return classify_failed_connect(authority);
                    }
                    let Some(authority) = authority else {
                        return Ok(NativeConnectOutcome::NoOwner);
                    };
                    if try_lock(authority.as_raw_fd())? {
                        unlock(authority.as_raw_fd())?;
                        return Ok(NativeConnectOutcome::NoOwner);
                    }
                    let lifetime_authority_id = authority_id(&authority)?;
                    Ok(NativeConnectOutcome::Connected(
                        super::NativeEmbeddingStream {
                            inner: Stream::new(
                                stream,
                                TransportIdentity {
                                    endpoint_namespace_id: runtime.endpoint_namespace_id.clone(),
                                    lifetime_authority_id,
                                    listener_id,
                                    peer_verified: true,
                                    peer_pid: peer.pid,
                                    peer_process_start_id: Some(peer_process_start_id),
                                },
                            )?,
                        },
                    ))
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        ErrorKind::NotFound
                            | ErrorKind::ConnectionRefused
                            | ErrorKind::ConnectionReset
                            | ErrorKind::TimedOut
                    ) =>
                {
                    classify_failed_connect(authority)
                }
                Err(error) => Err(error).context("connect to embedding server"),
            },
            Ok(None) => classify_failed_connect(authority),
            Err(error) => Err(error),
        }
    }

    fn classify_failed_connect(authority: Option<File>) -> Result<NativeConnectOutcome> {
        let Some(authority) = authority else {
            return Ok(NativeConnectOutcome::NoOwner);
        };
        if try_lock(authority.as_raw_fd())? {
            unlock(authority.as_raw_fd())?;
            Ok(NativeConnectOutcome::NoOwner)
        } else {
            Ok(NativeConnectOutcome::OwnerUnresponsive)
        }
    }

    pub(super) fn bind() -> Result<BindOutcome> {
        let paths = runtime_paths()?;
        let authority_directory = authority_directory(&paths)?;
        let authority = open_authority(&authority_directory, true)?
            .context("embedding lifetime authority file was not created")?;
        if !try_lock(authority.as_raw_fd())? {
            return Ok(BindOutcome::AlreadyOwned);
        }
        let runtime = runtime_directory(&paths, &authority_directory, true)?
            .context("embedding runtime socket directory was not created")?;

        let socket_path = runtime.path.join(&paths.socket_name);
        remove_stale_socket(&runtime, &socket_path, &paths.socket_name)?;
        let listener = UnixListener::bind(&socket_path)
            .with_context(|| format!("bind embedding socket {}", socket_path.display()))?;
        fs::set_permissions(&socket_path, fs::Permissions::from_mode(PRIVATE_FILE_MODE))
            .with_context(|| format!("secure embedding socket {}", socket_path.display()))?;
        let listener_id = validate_socket_path(&socket_path, runtime.uid)?
            .context("bound embedding socket disappeared")?;
        let socket_identity = socket_identity(&socket_path)?;
        ensure_runtime_directory_matches(&runtime)?;
        listener
            .set_nonblocking(true)
            .context("set embedding listener nonblocking")?;
        let lifetime_authority_id = authority_id(&authority)?;
        let identity = TransportIdentity {
            endpoint_namespace_id: runtime.endpoint_namespace_id.clone(),
            lifetime_authority_id,
            listener_id,
            peer_verified: true,
            peer_pid: None,
            peer_process_start_id: None,
        };
        Ok(BindOutcome::Bound(Listener {
            listener,
            runtime,
            authority_directory,
            authority,
            socket_name: paths.socket_name,
            socket_identity,
            identity,
        }))
    }

    pub(super) fn detach_command(command: &mut Command) {
        use std::os::unix::process::CommandExt;

        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }

    pub(super) fn clock_api() -> &'static str {
        #[cfg(target_os = "macos")]
        {
            "mach_absolute_time"
        }
        #[cfg(target_os = "linux")]
        {
            "CLOCK_MONOTONIC"
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            "std::time::Instant"
        }
    }

    pub(super) fn inclusive_clock_api() -> &'static str {
        #[cfg(target_os = "macos")]
        {
            "mach_continuous_time"
        }
        #[cfg(target_os = "linux")]
        {
            "CLOCK_BOOTTIME"
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            "unsupported"
        }
    }

    pub(super) fn inclusive_now_ns() -> Result<u64> {
        #[cfg(target_os = "linux")]
        {
            let mut value = libc::timespec {
                tv_sec: 0,
                tv_nsec: 0,
            };
            if unsafe { libc::clock_gettime(libc::CLOCK_BOOTTIME, &mut value) } != 0 {
                return Err(std::io::Error::last_os_error()).context("read CLOCK_BOOTTIME");
            }
            Ok((value.tv_sec.max(0) as u64)
                .saturating_mul(1_000_000_000)
                .saturating_add(value.tv_nsec.max(0) as u64))
        }
        #[cfg(target_os = "macos")]
        {
            let absolute = unsafe { mach_continuous_time() };
            let mut timebase = MachTimebaseInfo { numer: 0, denom: 0 };
            if unsafe { mach_timebase_info(&mut timebase) } != 0 || timebase.denom == 0 {
                return Err(std::io::Error::last_os_error())
                    .context("query mach continuous timebase");
            }
            Ok(
                (u128::from(absolute) * u128::from(timebase.numer) / u128::from(timebase.denom))
                    .min(u128::from(u64::MAX)) as u64,
            )
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            bail!("inclusive monotonic clock is unsupported on this Unix platform")
        }
    }

    pub(super) fn boot_id() -> Result<String> {
        #[cfg(target_os = "linux")]
        {
            let id = fs::read_to_string("/proc/sys/kernel/random/boot_id")
                .context("read Linux boot id")?;
            let id = id.trim();
            if id.is_empty() {
                bail!("Linux boot id was empty");
            }
            Ok(id.to_string())
        }
        #[cfg(target_os = "macos")]
        {
            macos_boot_session_uuid()
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            bail!("boot identity is unsupported on this Unix platform")
        }
    }

    pub(super) fn awake_now_ns() -> u64 {
        #[cfg(target_os = "linux")]
        {
            let mut value = libc::timespec {
                tv_sec: 0,
                tv_nsec: 0,
            };
            let success = unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut value) } == 0;
            fail_closed_clock_value(
                success.then(|| {
                    (value.tv_sec.max(0) as u64)
                        .saturating_mul(1_000_000_000)
                        .saturating_add(value.tv_nsec.max(0) as u64)
                }),
                "clock_gettime(CLOCK_MONOTONIC)",
            )
        }
        #[cfg(target_os = "macos")]
        {
            let absolute = unsafe { mach_absolute_time() };
            let mut timebase = MachTimebaseInfo { numer: 0, denom: 0 };
            let success = unsafe { mach_timebase_info(&mut timebase) } == 0 && timebase.denom != 0;
            fail_closed_clock_value(
                success.then(|| {
                    (u128::from(absolute) * u128::from(timebase.numer) / u128::from(timebase.denom))
                        .min(u128::from(u64::MAX)) as u64
                }),
                "mach_timebase_info",
            )
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            fail_closed_clock_value(None, "unsupported Unix awake clock")
        }
    }

    fn fail_closed_clock_value(value: Option<u64>, api: &str) -> u64 {
        value.unwrap_or_else(|| {
            panic!("embedding awake monotonic clock failed after validation: {api}")
        })
    }

    pub(super) fn clock_resolution_ns() -> Result<u64> {
        #[cfg(target_os = "linux")]
        {
            let mut value = libc::timespec {
                tv_sec: 0,
                tv_nsec: 0,
            };
            if unsafe { libc::clock_getres(libc::CLOCK_MONOTONIC, &mut value) } != 0 {
                return Err(std::io::Error::last_os_error())
                    .context("query CLOCK_MONOTONIC resolution");
            }
            Ok((value.tv_sec.max(0) as u64)
                .saturating_mul(1_000_000_000)
                .saturating_add(value.tv_nsec.max(0) as u64)
                .max(1))
        }
        #[cfg(target_os = "macos")]
        {
            let mut timebase = MachTimebaseInfo { numer: 0, denom: 0 };
            if unsafe { mach_timebase_info(&mut timebase) } != 0 || timebase.denom == 0 {
                return Err(std::io::Error::last_os_error())
                    .context("query mach timebase resolution");
            }
            Ok((u64::from(timebase.numer) / u64::from(timebase.denom)).max(1))
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            Ok(1)
        }
    }

    fn bounded_connect(path: &Path, budget: Duration) -> std::io::Result<UnixStream> {
        validate_unix_socket_path(path)?;
        let path_bytes = path.as_os_str().as_bytes();
        let mut address = unsafe { std::mem::zeroed::<libc::sockaddr_un>() };
        let path_offset =
            std::ptr::addr_of!(address.sun_path) as usize - std::ptr::addr_of!(address) as usize;
        address.sun_family = libc::AF_UNIX as libc::sa_family_t;
        unsafe {
            std::ptr::copy_nonoverlapping(
                path_bytes.as_ptr(),
                address.sun_path.as_mut_ptr().cast(),
                path_bytes.len(),
            );
        }
        let address_len = (path_offset + path_bytes.len() + 1) as libc::socklen_t;
        let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        if unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let initial_flags = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFL) };
        if initial_flags < 0
            || unsafe {
                libc::fcntl(
                    fd.as_raw_fd(),
                    libc::F_SETFL,
                    initial_flags | libc::O_NONBLOCK,
                )
            } < 0
        {
            return Err(std::io::Error::last_os_error());
        }
        let connected = unsafe {
            libc::connect(
                fd.as_raw_fd(),
                (&address as *const libc::sockaddr_un).cast(),
                address_len,
            )
        };
        if connected != 0 {
            let error = std::io::Error::last_os_error();
            if !error.raw_os_error().is_some_and(|code| {
                code == libc::EINPROGRESS || code == libc::EAGAIN || code == libc::EWOULDBLOCK
            }) {
                return Err(error);
            }
            let started = awake_now_ns();
            loop {
                let elapsed = Duration::from_nanos(awake_now_ns().saturating_sub(started));
                if elapsed >= budget {
                    return Err(std::io::Error::new(
                        ErrorKind::TimedOut,
                        "embedding socket connect timed out",
                    ));
                }
                let remaining = budget.saturating_sub(elapsed);
                let poll_ms = remaining.as_millis().clamp(1, 50) as i32;
                let mut poll = libc::pollfd {
                    fd: fd.as_raw_fd(),
                    events: libc::POLLOUT,
                    revents: 0,
                };
                let result = unsafe { libc::poll(&mut poll, 1, poll_ms) };
                if result < 0 {
                    let poll_error = std::io::Error::last_os_error();
                    if poll_error.kind() == ErrorKind::Interrupted {
                        continue;
                    }
                    return Err(poll_error);
                }
                if result == 0 {
                    continue;
                }
                let mut socket_error = 0_i32;
                let mut length = std::mem::size_of::<i32>() as libc::socklen_t;
                if unsafe {
                    libc::getsockopt(
                        fd.as_raw_fd(),
                        libc::SOL_SOCKET,
                        libc::SO_ERROR,
                        (&mut socket_error as *mut i32).cast(),
                        &mut length,
                    )
                } != 0
                {
                    return Err(std::io::Error::last_os_error());
                }
                if socket_error != 0 {
                    return Err(std::io::Error::from_raw_os_error(socket_error));
                }
                break;
            }
        }
        let flags = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFL) };
        if flags < 0
            || unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, flags & !libc::O_NONBLOCK) } < 0
        {
            return Err(std::io::Error::last_os_error());
        }
        Ok(unsafe { UnixStream::from_raw_fd(fd.into_raw_fd()) })
    }

    fn validate_unix_socket_path(path: &Path) -> std::io::Result<()> {
        let path_bytes = path.as_os_str().as_bytes();
        if path_bytes.contains(&0) {
            return Err(std::io::Error::new(
                ErrorKind::InvalidInput,
                "embedding_endpoint_path_invalid: socket path contains NUL",
            ));
        }
        let capacity = unsafe { std::mem::zeroed::<libc::sockaddr_un>() }
            .sun_path
            .len();
        if path_bytes.len() + 1 > capacity {
            return Err(std::io::Error::new(
                ErrorKind::InvalidInput,
                format!(
                    "embedding_endpoint_path_too_long: socket path uses {} bytes but the platform limit is {}",
                    path_bytes.len(),
                    capacity.saturating_sub(1),
                ),
            ));
        }
        Ok(())
    }

    fn timeout_ns(timeout: Option<Duration>) -> u64 {
        timeout
            .map(|timeout| timeout.as_nanos().min(u128::from(NO_TIMEOUT - 1)) as u64)
            .unwrap_or(NO_TIMEOUT)
    }

    fn poll_stream(
        fd: RawFd,
        events: i16,
        started_ns: u64,
        timeout_ns: u64,
        operation: &str,
    ) -> std::io::Result<()> {
        let elapsed_ns = awake_now_ns().saturating_sub(started_ns);
        if timeout_ns != NO_TIMEOUT && elapsed_ns >= timeout_ns {
            return Err(std::io::Error::new(
                ErrorKind::TimedOut,
                format!("embedding Unix stream {operation} timed out"),
            ));
        }
        let remaining = if timeout_ns == NO_TIMEOUT {
            STREAM_IO_POLL
        } else {
            Duration::from_nanos(timeout_ns.saturating_sub(elapsed_ns)).min(STREAM_IO_POLL)
        };
        let mut poll = libc::pollfd {
            fd,
            events,
            revents: 0,
        };
        let poll_ms = remaining.as_millis().max(1).min(i32::MAX as u128) as i32;
        let result = unsafe { libc::poll(&mut poll, 1, poll_ms) };
        if result < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == ErrorKind::Interrupted {
                return Ok(());
            }
            return Err(error);
        }
        Ok(())
    }

    fn runtime_directory(
        paths: &RuntimePaths,
        authority: &RuntimeDirectory,
        create: bool,
    ) -> Result<Option<RuntimeDirectory>> {
        let Some(socket_base) = validate_private_directory(&paths.socket_base, false)? else {
            bail!(
                "embedding_runtime_authority_unavailable: private socket base {} does not exist",
                paths.socket_base.display()
            );
        };
        let server_dir = socket_base.path.join(&paths.server_dir_name);
        let socket_path = server_dir.join(&paths.socket_name);
        validate_unix_socket_path(&socket_path).map_err(anyhow::Error::new)?;
        if create {
            match DirBuilder::new().mode(PRIVATE_DIR_MODE).create(&server_dir) {
                Ok(()) => {}
                Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!(
                            "create embedding runtime directory {}",
                            server_dir.display()
                        )
                    });
                }
            }
        }
        let Some(mut runtime) = validate_private_directory(&server_dir, false)? else {
            return Ok(None);
        };
        if runtime.uid != socket_base.uid || runtime.uid != authority.uid {
            bail!("embedding_runtime_authority_untrusted: socket and authority owners differ");
        }
        runtime.endpoint_namespace_id = sha256_fields(&[
            ENDPOINT_NAMESPACE.as_bytes(),
            paths.namespace_salt.as_bytes(),
            runtime.uid.to_string().as_bytes(),
            authority.dev.to_string().as_bytes(),
            authority.ino.to_string().as_bytes(),
            socket_base.dev.to_string().as_bytes(),
            socket_base.ino.to_string().as_bytes(),
            runtime.dev.to_string().as_bytes(),
            runtime.ino.to_string().as_bytes(),
        ]);
        ensure_runtime_directory_matches(authority)?;
        ensure_runtime_directory_matches(&socket_base)?;
        Ok(Some(runtime))
    }

    fn runtime_paths() -> Result<RuntimePaths> {
        match (
            std::env::var_os(QUALIFICATION_DIR_ENV),
            std::env::var_os(QUALIFICATION_NONCE_ENV),
        ) {
            (None, None) => platform_runtime_paths(),
            (Some(dir), Some(nonce)) => qualification_runtime_paths(
                PathBuf::from(dir),
                &nonce.to_string_lossy(),
                platform_runtime_paths()?.socket_base,
            ),
            _ => bail!(
                "embedding_qualification_gate_incomplete: both {QUALIFICATION_DIR_ENV} and {QUALIFICATION_NONCE_ENV} are required"
            ),
        }
    }

    pub(super) fn executable_attestation_store() -> Result<Option<NativeExecutableAttestationStore>>
    {
        let paths = runtime_paths()?;
        let authority = authority_directory(&paths)?;
        let endpoint_namespace_id = sha256_fields(&[
            b"codestory-executable-attestation-authority-v1",
            ENDPOINT_NAMESPACE.as_bytes(),
            paths.namespace_salt.as_bytes(),
            authority.dev.to_string().as_bytes(),
            authority.ino.to_string().as_bytes(),
            authority.uid.to_string().as_bytes(),
        ]);
        let file_name = format!(
            "executable-attestation-{}.json",
            &endpoint_namespace_id[..32]
        );
        Ok(Some(NativeExecutableAttestationStore {
            authority,
            endpoint_namespace_id,
            file_name,
        }))
    }

    #[cfg(test)]
    pub(super) fn test_executable_attestation_store(
        authority_path: &Path,
        endpoint_namespace_id: &str,
    ) -> Result<NativeExecutableAttestationStore> {
        let authority = validate_private_directory(authority_path, false)?
            .context("test executable attestation authority does not exist")?;
        Ok(NativeExecutableAttestationStore {
            authority,
            endpoint_namespace_id: endpoint_namespace_id.into(),
            file_name: format!(
                "executable-attestation-{}.json",
                &sha256_fields(&[endpoint_namespace_id.as_bytes()])[..32]
            ),
        })
    }

    #[cfg(test)]
    impl NativeExecutableAttestationStore {
        pub(super) fn test_path(&self) -> PathBuf {
            self.authority.path.join(&self.file_name)
        }
    }

    #[cfg(target_os = "linux")]
    fn platform_runtime_paths() -> Result<RuntimePaths> {
        let path = std::env::var_os("XDG_RUNTIME_DIR")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .context(
                "embedding_runtime_authority_unavailable: XDG_RUNTIME_DIR is required on Linux",
            )?;
        if !path.is_absolute() {
            bail!("embedding_runtime_authority_untrusted: XDG_RUNTIME_DIR must be absolute");
        }
        Ok(RuntimePaths {
            socket_base: path.clone(),
            authority_directory: path,
            namespace_salt: "linux-xdg-runtime".into(),
            server_dir_name: SERVER_DIR_NAME.into(),
            socket_name: SOCKET_NAME.into(),
            expected_authority_identity: None,
            authority_is_private: true,
            authority_has_fixed_parent: true,
        })
    }

    #[cfg(target_os = "macos")]
    fn platform_runtime_paths() -> Result<RuntimePaths> {
        let required =
            unsafe { libc::confstr(libc::_CS_DARWIN_USER_TEMP_DIR, std::ptr::null_mut(), 0) };
        if required == 0 {
            return Err(std::io::Error::last_os_error())
                .context("resolve _CS_DARWIN_USER_TEMP_DIR length");
        }
        let mut bytes = vec![0_u8; required];
        let written = unsafe {
            libc::confstr(
                libc::_CS_DARWIN_USER_TEMP_DIR,
                bytes.as_mut_ptr().cast(),
                bytes.len(),
            )
        };
        if written == 0 || written > bytes.len() {
            return Err(std::io::Error::last_os_error())
                .context("resolve _CS_DARWIN_USER_TEMP_DIR");
        }
        let cstr = CStr::from_bytes_until_nul(&bytes).context("parse _CS_DARWIN_USER_TEMP_DIR")?;
        let path = PathBuf::from(OsStr::from_bytes(cstr.to_bytes()));
        if !path.is_absolute() {
            bail!(
                "embedding_runtime_authority_untrusted: _CS_DARWIN_USER_TEMP_DIR must be absolute"
            );
        }
        let authority_directory = path
            .parent()
            .context("_CS_DARWIN_USER_TEMP_DIR has no anchored parent")?
            .to_path_buf();
        Ok(RuntimePaths {
            socket_base: path,
            authority_directory,
            namespace_salt: "macos-darwin-user-temp".into(),
            server_dir_name: SERVER_DIR_NAME.into(),
            socket_name: SOCKET_NAME.into(),
            expected_authority_identity: None,
            authority_is_private: false,
            authority_has_fixed_parent: true,
        })
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    fn platform_runtime_paths() -> Result<RuntimePaths> {
        bail!("per-user embedding server is unsupported on this Unix platform")
    }

    fn qualification_runtime_paths(
        authority_directory: PathBuf,
        nonce: &str,
        socket_base: PathBuf,
    ) -> Result<RuntimePaths> {
        if nonce.is_empty()
            || nonce.len() > 64
            || !nonce
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            bail!("embedding_qualification_nonce_invalid");
        }
        if !authority_directory.is_absolute() {
            bail!("embedding_qualification_directory_not_absolute");
        }
        let authority = validate_private_directory(&authority_directory, false)?
            .context("embedding qualification directory does not exist")?;
        let expected_authority_identity = (authority.dev, authority.ino, authority.uid);
        let namespace = sha256_fields(&[
            b"codestory-embedding-qualification-endpoint-v1",
            nonce.as_bytes(),
            authority.dev.to_string().as_bytes(),
            authority.ino.to_string().as_bytes(),
            authority.uid.to_string().as_bytes(),
        ]);
        Ok(RuntimePaths {
            socket_base,
            authority_directory,
            namespace_salt: format!("qualification:{namespace}"),
            server_dir_name: QUALIFICATION_SERVER_DIR_NAME.into(),
            socket_name: format!("q-{}.sock", &namespace[..32]),
            expected_authority_identity: Some(expected_authority_identity),
            authority_is_private: true,
            authority_has_fixed_parent: false,
        })
    }

    fn authority_directory(paths: &RuntimePaths) -> Result<RuntimeDirectory> {
        let directory = validate_owned_directory(
            &paths.authority_directory,
            if paths.authority_is_private {
                0o077
            } else {
                0o022
            },
        )?
        .with_context(|| {
            format!(
                "embedding authority directory {} does not exist",
                paths.authority_directory.display()
            )
        })?;
        if let Some(expected) = paths.expected_authority_identity
            && expected != (directory.dev, directory.ino, directory.uid)
        {
            bail!(
                "embedding_runtime_authority_replaced: qualification authority directory changed"
            );
        }
        if paths.authority_has_fixed_parent {
            validate_fixed_parent(&directory)?;
        }
        Ok(directory)
    }

    fn validate_fixed_parent(directory: &RuntimeDirectory) -> Result<()> {
        let parent = directory.path.parent().with_context(|| {
            format!(
                "embedding authority directory {} has no parent",
                directory.path.display()
            )
        })?;
        let metadata = fs::symlink_metadata(parent)
            .with_context(|| format!("inspect fixed authority parent {}", parent.display()))?;
        if metadata.file_type().is_symlink()
            || !metadata.is_dir()
            || metadata.uid() != 0
            || metadata.mode() & 0o022 != 0
        {
            bail!(
                "embedding_runtime_authority_untrusted: {} is not a root-owned non-writable parent",
                parent.display()
            );
        }
        let handle = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .open(parent)
            .with_context(|| format!("open fixed authority parent {}", parent.display()))?;
        let opened = handle.metadata()?;
        if opened.dev() != metadata.dev() || opened.ino() != metadata.ino() {
            bail!("embedding_runtime_authority_replaced: fixed parent changed while opened");
        }
        ensure_runtime_directory_matches(directory)
    }

    fn validate_private_directory(path: &Path, create: bool) -> Result<Option<RuntimeDirectory>> {
        if create && !path.exists() {
            DirBuilder::new()
                .mode(PRIVATE_DIR_MODE)
                .create(path)
                .with_context(|| format!("create private directory {}", path.display()))?;
        }
        validate_owned_directory(path, 0o077)
    }

    fn validate_owned_directory(
        path: &Path,
        forbidden_mode_bits: u32,
    ) -> Result<Option<RuntimeDirectory>> {
        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("inspect private directory {}", path.display()));
            }
        };
        if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
            bail!(
                "embedding_runtime_authority_untrusted: {} is not a real directory",
                path.display()
            );
        }
        let uid = unsafe { libc::geteuid() };
        if metadata.uid() != uid {
            bail!(
                "embedding_runtime_authority_untrusted: {} is owned by uid {}, expected {}",
                path.display(),
                metadata.uid(),
                uid
            );
        }
        if metadata.mode() & forbidden_mode_bits != 0 {
            bail!(
                "embedding_runtime_authority_untrusted: {} has forbidden write or access bits {:o}",
                path.display(),
                metadata.mode() & forbidden_mode_bits
            );
        }
        let handle = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .open(path)
            .with_context(|| format!("open private directory {}", path.display()))?;
        let opened = handle.metadata()?;
        if opened.dev() != metadata.dev() || opened.ino() != metadata.ino() {
            bail!(
                "embedding_runtime_authority_replaced: {} changed while it was opened",
                path.display()
            );
        }
        Ok(Some(RuntimeDirectory {
            path: path.to_path_buf(),
            handle,
            dev: metadata.dev(),
            ino: metadata.ino(),
            uid,
            forbidden_mode_bits,
            endpoint_namespace_id: String::new(),
        }))
    }

    fn ensure_runtime_directory_matches(runtime: &RuntimeDirectory) -> Result<()> {
        if runtime_directory_matches(runtime) {
            Ok(())
        } else {
            bail!(
                "embedding_runtime_authority_replaced: {} no longer names the held private directory",
                runtime.path.display()
            )
        }
    }

    fn runtime_directory_matches(runtime: &RuntimeDirectory) -> bool {
        let Ok(path_metadata) = fs::symlink_metadata(&runtime.path) else {
            return false;
        };
        let Ok(handle_metadata) = runtime.handle.metadata() else {
            return false;
        };
        !path_metadata.file_type().is_symlink()
            && path_metadata.is_dir()
            && path_metadata.dev() == runtime.dev
            && path_metadata.ino() == runtime.ino
            && handle_metadata.dev() == runtime.dev
            && handle_metadata.ino() == runtime.ino
            && path_metadata.uid() == runtime.uid
            && path_metadata.mode() & runtime.forbidden_mode_bits == 0
    }

    fn open_authority(runtime: &RuntimeDirectory, create: bool) -> Result<Option<File>> {
        ensure_runtime_directory_matches(runtime)?;
        let name = UnixCString::new(LOCK_NAME).expect("static authority name");
        let flags = libc::O_RDWR
            | libc::O_CLOEXEC
            | libc::O_NOFOLLOW
            | if create { libc::O_CREAT } else { 0 };
        let fd = unsafe {
            libc::openat(
                runtime.handle.as_raw_fd(),
                name.as_ptr(),
                flags,
                PRIVATE_FILE_MODE,
            )
        };
        if fd < 0 {
            let error = std::io::Error::last_os_error();
            if !create && error.kind() == ErrorKind::NotFound {
                return Ok(None);
            }
            return Err(error).context("open handle-relative embedding authority");
        }
        let file = unsafe { File::from_raw_fd(fd) };
        let metadata = file.metadata()?;
        if !metadata.file_type().is_file()
            || metadata.uid() != runtime.uid
            || metadata.nlink() != 1
            || metadata.mode() & 0o077 != 0
        {
            bail!("embedding_lifetime_authority_untrusted");
        }
        let mut path_stat = unsafe { std::mem::zeroed::<libc::stat>() };
        if unsafe {
            libc::fstatat(
                runtime.handle.as_raw_fd(),
                name.as_ptr(),
                &mut path_stat,
                libc::AT_SYMLINK_NOFOLLOW,
            )
        } != 0
        {
            return Err(std::io::Error::last_os_error())
                .context("reinspect handle-relative embedding authority");
        }
        if path_stat.st_mode & libc::S_IFMT != libc::S_IFREG
            || path_stat.st_dev as u64 != metadata.dev()
            || path_stat.st_ino as u64 != metadata.ino()
            || path_stat.st_uid != runtime.uid
        {
            bail!("embedding_lifetime_authority_replaced");
        }
        ensure_runtime_directory_matches(runtime)?;
        Ok(Some(file))
    }

    fn try_lock(fd: RawFd) -> Result<bool> {
        let result = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
        if result == 0 {
            return Ok(true);
        }
        let error = std::io::Error::last_os_error();
        if error
            .raw_os_error()
            .is_some_and(|code| code == libc::EWOULDBLOCK || code == libc::EAGAIN)
        {
            Ok(false)
        } else {
            Err(error).context("acquire embedding lifetime flock")
        }
    }

    fn unlock(fd: RawFd) -> Result<()> {
        if unsafe { libc::flock(fd, libc::LOCK_UN) } == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error()).context("release embedding lifetime flock")
        }
    }

    fn authority_id(file: &File) -> Result<String> {
        let metadata = file.metadata()?;
        Ok(sha256_fields(&[
            b"unix-flock-v1",
            metadata.dev().to_string().as_bytes(),
            metadata.ino().to_string().as_bytes(),
            metadata.uid().to_string().as_bytes(),
        ]))
    }

    fn socket_identity(path: &Path) -> Result<(u64, u64)> {
        let metadata = fs::symlink_metadata(path)
            .with_context(|| format!("inspect embedding socket {}", path.display()))?;
        if !metadata.file_type().is_socket() {
            bail!("embedding_endpoint_untrusted: endpoint is not a Unix socket");
        }
        Ok((metadata.dev(), metadata.ino()))
    }

    fn validate_socket_path(path: &Path, uid: u32) -> Result<Option<String>> {
        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error).context("inspect embedding endpoint"),
        };
        if metadata.file_type().is_symlink()
            || !metadata.file_type().is_socket()
            || metadata.uid() != uid
            || metadata.mode() & 0o077 != 0
        {
            bail!("embedding_endpoint_untrusted: endpoint type, owner, or mode is invalid");
        }
        Ok(Some(sha256_fields(&[
            b"unix-listener-v1",
            metadata.dev().to_string().as_bytes(),
            metadata.ino().to_string().as_bytes(),
            metadata.uid().to_string().as_bytes(),
        ])))
    }

    fn remove_stale_socket(
        runtime: &RuntimeDirectory,
        path: &Path,
        socket_name: &str,
    ) -> Result<()> {
        ensure_runtime_directory_matches(runtime)?;
        let Some(path_identity) = validate_socket_path(path, runtime.uid)? else {
            return Ok(());
        };
        let held_identity = socket_identity_at(runtime, socket_name)?
            .context("embedding stale socket disappeared before removal")?;
        let held_listener_id = sha256_fields(&[
            b"unix-listener-v1",
            held_identity.0.to_string().as_bytes(),
            held_identity.1.to_string().as_bytes(),
            runtime.uid.to_string().as_bytes(),
        ]);
        if held_listener_id != path_identity {
            bail!("embedding_endpoint_replaced: stale socket identity changed before removal");
        }
        unlink_socket_at(runtime, socket_name)
            .with_context(|| format!("remove stale embedding socket {}", path.display()))?;
        if socket_identity_at(runtime, socket_name)?.is_some() {
            bail!("embedding_endpoint_replaced: stale socket removal did not clear the held entry");
        }
        ensure_runtime_directory_matches(runtime)
    }

    fn socket_identity_at(
        runtime: &RuntimeDirectory,
        socket_name: &str,
    ) -> Result<Option<(u64, u64)>> {
        let name = UnixCString::new(socket_name).context("embedding endpoint name contains NUL")?;
        let mut stat = unsafe { std::mem::zeroed::<libc::stat>() };
        if unsafe {
            libc::fstatat(
                runtime.handle.as_raw_fd(),
                name.as_ptr(),
                &mut stat,
                libc::AT_SYMLINK_NOFOLLOW,
            )
        } != 0
        {
            let error = std::io::Error::last_os_error();
            if error.kind() == ErrorKind::NotFound {
                return Ok(None);
            }
            return Err(error).context("inspect embedding socket through held directory");
        }
        if stat.st_mode & libc::S_IFMT != libc::S_IFSOCK
            || stat.st_uid != runtime.uid
            || stat.st_mode & 0o077 != 0
        {
            bail!("embedding_endpoint_untrusted: held endpoint entry is not a private socket");
        }
        Ok(Some((stat.st_dev as u64, stat.st_ino as u64)))
    }

    fn unlink_socket_at(runtime: &RuntimeDirectory, socket_name: &str) -> Result<()> {
        let name = UnixCString::new(socket_name).context("embedding endpoint name contains NUL")?;
        if unsafe { libc::unlinkat(runtime.handle.as_raw_fd(), name.as_ptr(), 0) } == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
                .context("unlink embedding socket through held directory")
        }
    }

    #[derive(Debug, Clone, Copy)]
    struct PeerIdentity {
        uid: u32,
        pid: Option<u32>,
    }

    fn canonical_peer_process_start_identity(pid: u32) -> Result<String> {
        match codestory_retrieval::probe_process_start_identity(pid) {
            codestory_retrieval::ProcessStartProbe::Running { start_identity } => {
                Ok(start_identity)
            }
            codestory_retrieval::ProcessStartProbe::NotRunning => {
                bail!("authenticated Unix peer exited during identity capture")
            }
            codestory_retrieval::ProcessStartProbe::Unknown { reason } => {
                bail!("authenticated Unix peer process identity unavailable: {reason}")
            }
        }
    }

    #[cfg(target_os = "linux")]
    fn peer_process_start_identity(pid: u32) -> std::io::Result<Option<String>> {
        match codestory_retrieval::probe_process_start_identity(pid) {
            codestory_retrieval::ProcessStartProbe::Running { start_identity } => {
                Ok(Some(start_identity))
            }
            codestory_retrieval::ProcessStartProbe::NotRunning => Ok(None),
            codestory_retrieval::ProcessStartProbe::Unknown { reason } => Err(
                std::io::Error::other(format!("probe authenticated peer process: {reason}")),
            ),
        }
    }

    #[cfg(target_os = "macos")]
    fn peer_process_start_identity(pid: u32) -> std::io::Result<Option<String>> {
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
                Some(libc::ESRCH) | Some(libc::ENOENT) => Ok(None),
                _ => Err(error),
            };
        }
        if read != expected || info.pbi_pid != pid {
            return Err(std::io::Error::new(
                ErrorKind::InvalidData,
                "authenticated peer process identity was incomplete",
            ));
        }
        Ok(Some(format!(
            "macos-proc:{}:{}",
            info.pbi_start_tvsec, info.pbi_start_tvusec
        )))
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    fn peer_process_start_identity(_pid: u32) -> std::io::Result<Option<String>> {
        Err(std::io::Error::new(
            ErrorKind::Unsupported,
            "authenticated peer process identity is unsupported",
        ))
    }

    #[cfg(target_os = "linux")]
    fn peer_identity(fd: RawFd) -> Result<PeerIdentity> {
        let mut credentials = libc::ucred {
            pid: 0,
            uid: 0,
            gid: 0,
        };
        let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        let result = unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                (&mut credentials as *mut libc::ucred).cast(),
                &mut length,
            )
        };
        if result != 0 || length as usize != std::mem::size_of::<libc::ucred>() {
            return Err(std::io::Error::last_os_error())
                .context("authenticate embedding Unix peer");
        }
        if credentials.pid <= 0 {
            bail!("embedding_peer_identity_unavailable: peer pid was invalid");
        }
        Ok(PeerIdentity {
            uid: credentials.uid,
            pid: Some(credentials.pid as u32),
        })
    }

    #[cfg(target_os = "macos")]
    fn peer_identity(fd: RawFd) -> Result<PeerIdentity> {
        let mut uid = 0;
        let mut gid = 0;
        if unsafe { libc::getpeereid(fd, &mut uid, &mut gid) } != 0 {
            return Err(std::io::Error::last_os_error())
                .context("authenticate embedding Unix peer");
        }
        let mut pid = 0_i32;
        let mut length = std::mem::size_of::<i32>() as libc::socklen_t;
        if unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_LOCAL,
                libc::LOCAL_PEERPID,
                (&mut pid as *mut i32).cast(),
                &mut length,
            )
        } != 0
            || length as usize != std::mem::size_of::<i32>()
            || pid <= 0
        {
            return Err(std::io::Error::last_os_error())
                .context("read authenticated embedding Unix peer pid");
        }
        Ok(PeerIdentity {
            uid,
            pid: Some(pid as u32),
        })
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    fn peer_identity(_fd: RawFd) -> Result<PeerIdentity> {
        bail!("same-user peer authentication is unsupported on this Unix platform")
    }

    #[cfg(target_os = "macos")]
    fn macos_boot_session_uuid() -> Result<String> {
        let name = CString::new("kern.bootsessionuuid").expect("static sysctl name");
        let mut length = 0_usize;
        if unsafe {
            libc::sysctlbyname(
                name.as_ptr(),
                std::ptr::null_mut(),
                &mut length,
                std::ptr::null_mut(),
                0,
            )
        } != 0
        {
            return Err(std::io::Error::last_os_error()).context("query macOS boot session length");
        }
        let mut bytes = vec![0_u8; length];
        if unsafe {
            libc::sysctlbyname(
                name.as_ptr(),
                bytes.as_mut_ptr().cast(),
                &mut length,
                std::ptr::null_mut(),
                0,
            )
        } != 0
        {
            return Err(std::io::Error::last_os_error()).context("query macOS boot session");
        }
        let id = CStr::from_bytes_until_nul(&bytes)
            .context("parse macOS boot session")?
            .to_string_lossy()
            .into_owned();
        if id.is_empty() {
            bail!("macOS boot session identity was empty");
        }
        Ok(id)
    }

    #[cfg(target_os = "macos")]
    #[repr(C)]
    struct MachTimebaseInfo {
        numer: u32,
        denom: u32,
    }

    #[cfg(target_os = "macos")]
    unsafe extern "C" {
        fn mach_absolute_time() -> u64;
        fn mach_continuous_time() -> u64;
        fn mach_timebase_info(info: *mut MachTimebaseInfo) -> i32;
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::os::unix::fs::symlink;
        use std::time::Instant;

        fn private_directory(path: &Path) {
            fs::create_dir(path).expect("create private test directory");
            fs::set_permissions(path, fs::Permissions::from_mode(PRIVATE_DIR_MODE))
                .expect("secure private test directory");
        }

        #[test]
        fn qualification_endpoint_stays_short_and_shared_for_long_unicode_control_path() {
            let control_parent = tempfile::tempdir().expect("create qualification parent");
            fs::set_permissions(
                control_parent.path(),
                fs::Permissions::from_mode(PRIVATE_DIR_MODE),
            )
            .expect("secure qualification parent");
            let control = control_parent.path().join("qualification-ü-".repeat(12));
            private_directory(&control);

            let short_socket_base = tempfile::Builder::new()
                .prefix("csq")
                .tempdir_in("/tmp")
                .expect("create short socket base");
            fs::set_permissions(
                short_socket_base.path(),
                fs::Permissions::from_mode(PRIVATE_DIR_MODE),
            )
            .expect("secure short socket base");

            let legacy_path = control.join(SERVER_DIR_NAME).join(SOCKET_NAME);
            let legacy_error = validate_unix_socket_path(&legacy_path)
                .expect_err("the legacy qualification endpoint must exceed sun_path");
            assert!(
                legacy_error
                    .to_string()
                    .contains("embedding_endpoint_path_too_long")
            );

            let paths = qualification_runtime_paths(
                control.clone(),
                "long-unicode-path-regression",
                short_socket_base.path().to_path_buf(),
            )
            .expect("derive bounded qualification paths");
            let second_paths = qualification_runtime_paths(
                control,
                "long-unicode-path-regression",
                short_socket_base.path().to_path_buf(),
            )
            .expect("derive the same bounded qualification paths");
            assert_eq!(paths.server_dir_name, second_paths.server_dir_name);
            assert_eq!(paths.socket_name, second_paths.socket_name);
            assert_eq!(paths.namespace_salt, second_paths.namespace_salt);

            let authority_directory =
                authority_directory(&paths).expect("open qualification authority");
            let authority = open_authority(&authority_directory, true)
                .expect("create qualification authority")
                .expect("qualification authority exists");
            let contender = open_authority(&authority_directory, false)
                .expect("open contender authority")
                .expect("contender authority exists");
            assert!(try_lock(authority.as_raw_fd()).expect("win qualification election"));
            assert!(
                !try_lock(contender.as_raw_fd()).expect("contend qualification election"),
                "one qualification namespace must have one owner"
            );

            let runtime = runtime_directory(&paths, &authority_directory, true)
                .expect("create short runtime")
                .expect("short runtime exists");
            let socket_path = runtime.path.join(&paths.socket_name);
            validate_unix_socket_path(&socket_path).expect("derived endpoint fits sun_path");
            let listener = UnixListener::bind(&socket_path).expect("bind derived endpoint");
            fs::set_permissions(&socket_path, fs::Permissions::from_mode(PRIVATE_FILE_MODE))
                .expect("secure derived endpoint");
            let first_client = bounded_connect(&socket_path, Duration::from_secs(1))
                .expect("connect first qualification client");
            let second_client = bounded_connect(&socket_path, Duration::from_secs(1))
                .expect("connect second qualification client");
            drop((first_client, second_client));

            let socket_identity = socket_identity(&socket_path).expect("capture socket identity");
            let listener = Listener {
                listener,
                runtime,
                authority_directory,
                authority,
                socket_name: paths.socket_name,
                socket_identity,
                identity: TransportIdentity {
                    endpoint_namespace_id: "qualification-endpoint".into(),
                    lifetime_authority_id: "qualification-authority".into(),
                    listener_id: "qualification-listener".into(),
                    peer_verified: true,
                    peer_pid: None,
                    peer_process_start_id: None,
                },
            };
            drop(listener);
            assert!(!socket_path.exists(), "owner drop must unlink its endpoint");
            assert!(
                try_lock(contender.as_raw_fd()).expect("successor takes authority"),
                "the successor must acquire authority after owner drop"
            );
            unlock(contender.as_raw_fd()).expect("release successor authority");
        }

        fn test_identity(pid: u32) -> TransportIdentity {
            let peer_process_start_id =
                canonical_peer_process_start_identity(pid).expect("inspect test peer process");
            TransportIdentity {
                endpoint_namespace_id: "test-endpoint".into(),
                lifetime_authority_id: "test-authority".into(),
                listener_id: "test-listener".into(),
                peer_verified: true,
                peer_pid: Some(pid),
                peer_process_start_id: Some(peer_process_start_id),
            }
        }

        #[test]
        fn embedding_private_runtime_directory_rejects_symlinks_and_broad_modes() {
            let root = tempfile::tempdir().expect("create test root");
            fs::set_permissions(root.path(), fs::Permissions::from_mode(0o755))
                .expect("broaden test root");
            let mode_error = validate_private_directory(root.path(), false)
                .expect_err("broad runtime directory mode must fail closed");
            assert!(
                mode_error
                    .to_string()
                    .contains("has forbidden write or access bits")
            );

            fs::set_permissions(root.path(), fs::Permissions::from_mode(PRIVATE_DIR_MODE))
                .expect("secure test root");
            let target = root.path().join("target");
            private_directory(&target);
            let link = root.path().join("link");
            symlink(&target, &link).expect("create test symlink");
            let link_error = validate_private_directory(&link, false)
                .expect_err("symlink runtime directory must fail closed");
            assert!(link_error.to_string().contains("is not a real directory"));
        }

        #[test]
        fn embedding_only_lifetime_authority_winner_removes_a_stale_socket() {
            let root = tempfile::tempdir().expect("create test root");
            fs::set_permissions(root.path(), fs::Permissions::from_mode(PRIVATE_DIR_MODE))
                .expect("secure test root");
            let runtime_path = root.path().join("runtime");
            private_directory(&runtime_path);
            let runtime = validate_private_directory(&runtime_path, false)
                .expect("validate runtime")
                .expect("runtime exists");
            let first = open_authority(&runtime, true)
                .expect("open first authority")
                .expect("first authority exists");
            let second = open_authority(&runtime, false)
                .expect("open second authority")
                .expect("second authority exists");
            assert!(try_lock(first.as_raw_fd()).expect("lock first authority"));
            assert!(!try_lock(second.as_raw_fd()).expect("contend second authority"));

            let socket_path = runtime.path.join(SOCKET_NAME);
            let stale_listener = UnixListener::bind(&socket_path).expect("bind stale socket");
            fs::set_permissions(&socket_path, fs::Permissions::from_mode(PRIVATE_FILE_MODE))
                .expect("secure stale socket");
            assert!(
                socket_path.exists(),
                "loser must leave stale entry untouched"
            );

            unlock(first.as_raw_fd()).expect("release first authority");
            assert!(try_lock(second.as_raw_fd()).expect("second authority becomes winner"));
            remove_stale_socket(&runtime, &socket_path, SOCKET_NAME)
                .expect("winner removes stale socket");
            assert!(!socket_path.exists());
            unlock(second.as_raw_fd()).expect("release second authority");
            drop(stale_listener);
        }

        #[test]
        fn embedding_stale_cleanup_preserves_wrong_type_entries() {
            let root = tempfile::tempdir().expect("create test root");
            fs::set_permissions(root.path(), fs::Permissions::from_mode(PRIVATE_DIR_MODE))
                .expect("secure test root");
            let runtime_path = root.path().join("runtime");
            private_directory(&runtime_path);
            let runtime = validate_private_directory(&runtime_path, false)
                .expect("validate runtime")
                .expect("runtime exists");
            let authority = open_authority(&runtime, true)
                .expect("open authority")
                .expect("authority exists");
            assert!(try_lock(authority.as_raw_fd()).expect("lock authority"));
            let endpoint = runtime.path.join(SOCKET_NAME);
            File::create(&endpoint).expect("create wrong-type endpoint");
            fs::set_permissions(&endpoint, fs::Permissions::from_mode(PRIVATE_FILE_MODE))
                .expect("secure wrong-type endpoint");
            let error = remove_stale_socket(&runtime, &endpoint, SOCKET_NAME)
                .expect_err("wrong-type endpoint must fail closed");
            assert!(error.to_string().contains("embedding_endpoint_untrusted"));
            assert!(endpoint.is_file(), "wrong-type entry must be preserved");
            unlock(authority.as_raw_fd()).expect("release authority");
        }

        #[test]
        fn embedding_replaced_socket_directory_cannot_mint_a_second_authority() {
            let root = tempfile::tempdir().expect("create test root");
            fs::set_permissions(root.path(), fs::Permissions::from_mode(PRIVATE_DIR_MODE))
                .expect("secure test root");
            let authority_directory = validate_private_directory(root.path(), false)
                .expect("validate authority directory")
                .expect("authority directory exists");
            let original = root.path().join(SERVER_DIR_NAME);
            private_directory(&original);
            let first = open_authority(&authority_directory, true)
                .expect("open first authority")
                .expect("first authority exists");
            assert!(try_lock(first.as_raw_fd()).expect("lock first authority"));

            let moved = root.path().join("renamed-server");
            fs::rename(&original, &moved).expect("rename socket directory");
            private_directory(&original);
            let second = open_authority(&authority_directory, false)
                .expect("reopen anchored authority")
                .expect("anchored authority exists");
            assert!(
                !try_lock(second.as_raw_fd()).expect("contend anchored authority"),
                "replacing the socket directory must not create a second lifetime authority"
            );
            unlock(first.as_raw_fd()).expect("release first authority");
        }

        #[test]
        fn embedding_peer_liveness_fails_closed_on_process_start_identity_change() {
            let (stream, _peer) = UnixStream::pair().expect("create Unix stream pair");
            let mut stream =
                Stream::new(stream, test_identity(std::process::id())).expect("capture peer");
            assert!(stream.peer_is_alive().expect("probe live peer"));
            stream.peer_start_identity.push_str("-reused");
            assert!(
                !stream
                    .peer_is_alive()
                    .expect("reused process identity is not the authenticated peer")
            );
        }

        #[test]
        fn embedding_stream_timeout_is_bounded_by_awake_clock_polling() {
            let (stream, _peer) = UnixStream::pair().expect("create Unix stream pair");
            let mut stream =
                Stream::new(stream, test_identity(std::process::id())).expect("capture peer");
            stream
                .set_read_timeout(Some(Duration::from_millis(30)))
                .expect("set read timeout");
            let started = Instant::now();
            let error = stream
                .read(&mut [0_u8; 1])
                .expect_err("empty peer must time out");
            assert_eq!(error.kind(), ErrorKind::TimedOut);
            assert!(started.elapsed() >= Duration::from_millis(20));
            assert!(started.elapsed() < Duration::from_secs(2));
        }

        #[test]
        fn embedding_accept_poll_remains_bounded_without_peer_image_work() {
            let root = tempfile::tempdir().expect("create test root");
            fs::set_permissions(root.path(), fs::Permissions::from_mode(PRIVATE_DIR_MODE))
                .expect("secure test root");
            let authority_directory = validate_private_directory(root.path(), false)
                .expect("validate authority directory")
                .expect("authority directory exists");
            let runtime_path = root.path().join(SERVER_DIR_NAME);
            private_directory(&runtime_path);
            let runtime = validate_private_directory(&runtime_path, false)
                .expect("validate runtime directory")
                .expect("runtime directory exists");
            let authority = open_authority(&authority_directory, true)
                .expect("open authority")
                .expect("authority exists");
            assert!(try_lock(authority.as_raw_fd()).expect("lock authority"));
            let socket_path = runtime.path.join(SOCKET_NAME);
            let listener = UnixListener::bind(&socket_path).expect("bind test socket");
            fs::set_permissions(&socket_path, fs::Permissions::from_mode(PRIVATE_FILE_MODE))
                .expect("secure test socket");
            listener
                .set_nonblocking(true)
                .expect("set test listener nonblocking");
            let socket_identity = socket_identity(&socket_path).expect("socket identity");
            let listener = Listener {
                listener,
                runtime,
                authority_directory,
                authority,
                socket_name: SOCKET_NAME.into(),
                socket_identity,
                identity: TransportIdentity {
                    endpoint_namespace_id: "test-endpoint".into(),
                    lifetime_authority_id: "test-authority".into(),
                    listener_id: "test-listener".into(),
                    peer_verified: true,
                    peer_pid: None,
                    peer_process_start_id: None,
                },
            };
            let started = Instant::now();
            assert!(
                listener
                    .accept(Duration::from_millis(25))
                    .expect("bounded accept")
                    .is_none()
            );
            assert!(started.elapsed() >= Duration::from_millis(15));
            assert!(started.elapsed() < Duration::from_millis(250));
        }

        #[test]
        fn embedding_awake_clock_failure_panics_instead_of_disabling_deadlines() {
            assert_eq!(fail_closed_clock_value(Some(7), "test"), 7);
            let failure = std::panic::catch_unwind(|| fail_closed_clock_value(None, "test"));
            assert!(failure.is_err());
        }

        #[test]
        fn embedding_clock_labels_match_the_measurement_protocol() {
            #[cfg(target_os = "linux")]
            {
                assert_eq!(clock_api(), "CLOCK_MONOTONIC");
                assert_eq!(inclusive_clock_api(), "CLOCK_BOOTTIME");
            }
            #[cfg(target_os = "macos")]
            {
                assert_eq!(clock_api(), "mach_absolute_time");
                assert_eq!(inclusive_clock_api(), "mach_continuous_time");
            }
        }
    }
}

#[cfg(windows)]
mod platform {
    use super::{
        ENDPOINT_NAMESPACE, ExecutableAttestationStore, NativeConnectOutcome,
        QUALIFICATION_DIR_ENV, QUALIFICATION_NONCE_ENV, RetainedWindowsAuthorityState,
        TransportIdentity, awake_deadline_ns, classify_windows_data_pipe_open_error, sha256_fields,
    };
    use anyhow::{Context, Result, bail};
    use std::ffi::c_void;
    use std::io::{Read, Write};
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use std::path::PathBuf;
    use std::process::Command;
    use std::ptr::{null, null_mut};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;
    use windows_sys::Win32::Foundation::{
        ERROR_ACCESS_DENIED, ERROR_BROKEN_PIPE, ERROR_FILE_NOT_FOUND, ERROR_INSUFFICIENT_BUFFER,
        ERROR_NO_DATA, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED, ERROR_PIPE_LISTENING,
        ERROR_PIPE_NOT_CONNECTED, ERROR_SEM_TIMEOUT, FILETIME, GENERIC_ALL, GENERIC_READ,
        GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE, LocalFree, STILL_ACTIVE,
    };
    use windows_sys::Win32::Security::Authorization::{
        ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
        GetSecurityInfo, SDDL_REVISION_1, SE_KERNEL_OBJECT,
    };
    use windows_sys::Win32::Security::{
        ACCESS_ALLOWED_ACE, ACL, ACL_SIZE_INFORMATION, AclSizeInformation, CopySid,
        CreateWellKnownSid, DACL_SECURITY_INFORMATION, EqualSid, GetAce, GetAclInformation,
        GetLengthSid, GetTokenInformation, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID,
        SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser, WinLocalSystemSid,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ALL_ACCESS, FILE_FLAG_FIRST_PIPE_INSTANCE, OPEN_EXISTING,
        PIPE_ACCESS_DUPLEX, ReadFile, WriteFile,
    };
    use windows_sys::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, GetNamedPipeClientProcessId,
        GetNamedPipeServerProcessId, PIPE_NOWAIT, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
        PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT, SetNamedPipeHandleState,
        WaitNamedPipeW,
    };
    use windows_sys::Win32::System::SystemInformation::{GetSystemTimeAsFileTime, GetTickCount64};
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetCurrentProcessId, GetExitCodeProcess, OpenProcess, OpenProcessToken,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows_sys::Win32::System::WindowsProgramming::QueryUnbiasedInterruptTimePrecise;

    const NO_TIMEOUT: u64 = u64::MAX;
    const PIPE_BUFFER_BYTES: usize = 1024 * 1024;
    const PIPE_IO_POLL: Duration = Duration::from_millis(1);
    const PIPE_CONNECT_POLL: Duration = Duration::from_millis(25);
    const SYNCHRONIZE_ACCESS: u32 = 0x0010_0000;

    pub(super) struct NativeExecutableAttestationStore;

    impl ExecutableAttestationStore for NativeExecutableAttestationStore {
        fn endpoint_namespace_id(&self) -> &str {
            unreachable!("Windows executable attestations are never cached")
        }

        fn read(&self) -> Result<Option<Vec<u8>>> {
            unreachable!("Windows executable attestations are never cached")
        }

        fn publish(&self, _content: &[u8]) -> Result<()> {
            unreachable!("Windows executable attestations are never cached")
        }
    }

    pub(super) fn executable_attestation_store() -> Result<Option<NativeExecutableAttestationStore>>
    {
        Ok(None)
    }

    #[derive(Debug)]
    #[allow(clippy::large_enum_variant)]
    pub(super) enum BindOutcome {
        Bound(Listener),
        AlreadyOwned,
    }

    #[derive(Debug)]
    pub(super) struct Listener {
        pipe_name: Vec<u16>,
        security_sddl: Vec<u16>,
        current_sid: SidBuffer,
        _authority_server: OwnedHandle,
        _authority_client: OwnedHandle,
        identity: TransportIdentity,
    }

    impl Listener {
        pub(super) fn identity(&self) -> &TransportIdentity {
            &self.identity
        }

        pub(super) fn accept(&self, timeout: Duration) -> Result<Option<Stream>> {
            let handle = create_pipe_instance(&self.pipe_name, &self.security_sddl, false, true)?;
            let started = awake_now_ns();
            loop {
                if unsafe { ConnectNamedPipe(raw(&handle), null_mut()) } != 0 {
                    break;
                }
                let error = std::io::Error::last_os_error();
                match error.raw_os_error().map(|code| code as u32) {
                    Some(ERROR_PIPE_CONNECTED) => break,
                    Some(ERROR_PIPE_LISTENING) => {
                        let elapsed = Duration::from_nanos(awake_now_ns().saturating_sub(started));
                        if elapsed >= timeout {
                            return Ok(None);
                        }
                        std::thread::sleep(
                            Duration::from_millis(10).min(timeout.saturating_sub(elapsed)),
                        );
                    }
                    Some(ERROR_NO_DATA) => return Ok(None),
                    _ => return Err(error).context("accept embedding named-pipe connection"),
                }
            }
            authenticate_peer(handle, &self.current_sid, self.identity.clone())
                .map(Some)
                .context("embedding_peer_rejected")
        }
    }

    fn authenticate_peer(
        handle: OwnedHandle,
        current_sid: &SidBuffer,
        identity: TransportIdentity,
    ) -> Result<Stream> {
        validate_pipe_dacl(raw(&handle), current_sid)?;
        let peer_pid = named_pipe_client_pid(raw(&handle))?;
        validate_process_sid(peer_pid, current_sid)?;
        let peer_process_start_id = canonical_process_start_identity(peer_pid)?;
        Stream::new(
            handle,
            true,
            TransportIdentity {
                peer_pid: Some(peer_pid),
                peer_process_start_id: Some(peer_process_start_id),
                ..identity
            },
        )
    }

    #[derive(Debug)]
    pub(super) struct Stream {
        handle: OwnedHandle,
        peer_process: OwnedHandle,
        server_side: bool,
        identity: TransportIdentity,
        read_timeout_ns: AtomicU64,
        write_timeout_ns: AtomicU64,
    }

    impl Stream {
        fn new(
            handle: OwnedHandle,
            server_side: bool,
            identity: TransportIdentity,
        ) -> Result<Self> {
            let peer_pid = identity
                .peer_pid
                .context("authenticated Windows peer PID is unavailable")?;
            let peer_start_identity = canonical_process_start_identity(peer_pid)?;
            if identity.peer_process_start_id.as_deref() != Some(&peer_start_identity) {
                bail!("embedding_peer_process_identity_changed");
            }
            let peer_process = unsafe {
                OpenProcess(
                    PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE_ACCESS,
                    0,
                    peer_pid,
                )
            };
            if peer_process.is_null() {
                return Err(std::io::Error::last_os_error())
                    .with_context(|| format!("retain embedding peer process {peer_pid}"));
            }
            Ok(Self {
                handle,
                peer_process: unsafe { OwnedHandle::from_raw_handle(peer_process.cast()) },
                server_side,
                identity,
                read_timeout_ns: AtomicU64::new(NO_TIMEOUT),
                write_timeout_ns: AtomicU64::new(NO_TIMEOUT),
            })
        }

        pub(super) fn identity(&self) -> &TransportIdentity {
            &self.identity
        }

        pub(super) fn set_read_timeout(&self, timeout: Option<Duration>) -> std::io::Result<()> {
            self.read_timeout_ns
                .store(timeout_ns(timeout), Ordering::Release);
            Ok(())
        }

        pub(super) fn set_write_timeout(&self, timeout: Option<Duration>) -> std::io::Result<()> {
            self.write_timeout_ns
                .store(timeout_ns(timeout), Ordering::Release);
            Ok(())
        }

        pub(super) fn shutdown(&self) -> std::io::Result<()> {
            if self.server_side && unsafe { DisconnectNamedPipe(raw(&self.handle)) } == 0 {
                let error = std::io::Error::last_os_error();
                if error.raw_os_error().map(|code| code as u32) != Some(ERROR_NO_DATA) {
                    return Err(error);
                }
            }
            Ok(())
        }

        pub(super) fn finish_response_delivery(&self) -> std::io::Result<()> {
            if !self.server_side {
                return Ok(());
            }
            let started = awake_now_ns();
            loop {
                let mut unexpected = 0_u8;
                let mut read = 0_u32;
                if unsafe {
                    ReadFile(
                        raw(&self.handle),
                        (&mut unexpected as *mut u8).cast(),
                        1,
                        &mut read,
                        null_mut(),
                    )
                } != 0
                {
                    if read == 0 {
                        return Ok(());
                    }
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "embedding client wrote data after the final response",
                    ));
                }
                let error = std::io::Error::last_os_error();
                match error.raw_os_error().map(|code| code as u32) {
                    Some(ERROR_BROKEN_PIPE) | Some(ERROR_PIPE_NOT_CONNECTED) => return Ok(()),
                    Some(ERROR_NO_DATA) | Some(ERROR_PIPE_BUSY) | Some(ERROR_PIPE_LISTENING) => {
                        wait_pipe_io(
                            started,
                            self.read_timeout_ns.load(Ordering::Acquire),
                            "finish response delivery",
                        )?;
                    }
                    _ => return Err(normalize_windows_pipe_io_error(error)),
                }
            }
        }

        pub(super) fn peer_is_alive(&self) -> std::io::Result<bool> {
            Ok(self.peer_exit_code()?.is_none())
        }

        pub(super) fn peer_exit_code(&self) -> std::io::Result<Option<u32>> {
            let mut exit_code = 0_u32;
            if unsafe { GetExitCodeProcess(raw(&self.peer_process), &mut exit_code) } == 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok((exit_code != STILL_ACTIVE as u32).then_some(exit_code))
        }
    }

    impl Read for Stream {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            let amount = buffer.len().min(u32::MAX as usize) as u32;
            let started = awake_now_ns();
            loop {
                let mut read = 0_u32;
                if unsafe {
                    ReadFile(
                        raw(&self.handle),
                        buffer.as_mut_ptr(),
                        amount,
                        &mut read,
                        null_mut(),
                    )
                } != 0
                {
                    return Ok(read as usize);
                }
                let error = std::io::Error::last_os_error();
                if !error
                    .raw_os_error()
                    .map(|code| code as u32)
                    .is_some_and(|code| {
                        code == ERROR_NO_DATA
                            || code == ERROR_PIPE_BUSY
                            || code == ERROR_PIPE_LISTENING
                    })
                {
                    return Err(normalize_windows_pipe_io_error(error));
                }
                wait_pipe_io(
                    started,
                    self.read_timeout_ns.load(Ordering::Acquire),
                    "read",
                )?;
            }
        }
    }

    impl Write for Stream {
        fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
            if buffer.is_empty() {
                return Ok(0);
            }
            // PIPE_NOWAIT does not promise a partial write when the request is
            // larger than the configured pipe buffer. Bound each WriteFile
            // call to one buffer so write_all can make progress on protocol
            // payloads up to the larger frame limit.
            let amount = buffer.len().min(PIPE_BUFFER_BYTES) as u32;
            let started = awake_now_ns();
            loop {
                let mut written = 0_u32;
                if unsafe {
                    WriteFile(
                        raw(&self.handle),
                        buffer.as_ptr(),
                        amount,
                        &mut written,
                        null_mut(),
                    )
                } != 0
                {
                    if written != 0 {
                        return Ok(written as usize);
                    }
                    // A nonblocking byte pipe can report a successful
                    // zero-byte write while its kernel buffer is full. Keep
                    // the response inside the already configured finite write
                    // deadline instead of surfacing WriteZero to write_all.
                    wait_pipe_io(
                        started,
                        self.write_timeout_ns.load(Ordering::Acquire),
                        "write",
                    )?;
                    continue;
                }
                let error = std::io::Error::last_os_error();
                if !error
                    .raw_os_error()
                    .map(|code| code as u32)
                    .is_some_and(|code| code == ERROR_NO_DATA || code == ERROR_PIPE_BUSY)
                {
                    return Err(normalize_windows_pipe_io_error(error));
                }
                wait_pipe_io(
                    started,
                    self.write_timeout_ns.load(Ordering::Acquire),
                    "write",
                )?;
            }
        }

        fn flush(&mut self) -> std::io::Result<()> {
            // Synchronous WriteFile has already copied the frame into the
            // kernel pipe buffer. FlushFileBuffers waits for the peer to read
            // every byte and would bypass the awake-time request deadline.
            Ok(())
        }
    }

    impl Drop for Stream {
        fn drop(&mut self) {
            if self.server_side {
                let _ = unsafe { DisconnectNamedPipe(raw(&self.handle)) };
            }
        }
    }

    pub(super) fn connect(budget: Duration) -> Result<NativeConnectOutcome> {
        let started = awake_now_ns();
        let deadline_ns = awake_deadline_ns(started, budget);
        let current_sid = current_process_sid()?;
        let (pipe_name, endpoint_namespace_id) = pipe_name(&current_sid)?;
        let authority_pipe_name = authority_pipe_name(&endpoint_namespace_id);
        loop {
            if awake_now_ns() >= deadline_ns {
                return Ok(NativeConnectOutcome::OwnerUnresponsive);
            }
            let handle = unsafe {
                CreateFileW(
                    pipe_name.as_ptr(),
                    GENERIC_READ | GENERIC_WRITE,
                    0,
                    null(),
                    OPEN_EXISTING,
                    0,
                    null_mut(),
                )
            };
            if handle != INVALID_HANDLE_VALUE {
                let handle = unsafe { OwnedHandle::from_raw_handle(handle.cast()) };
                let nonblocking = PIPE_READMODE_BYTE | PIPE_NOWAIT;
                if unsafe { SetNamedPipeHandleState(raw(&handle), &nonblocking, null(), null()) }
                    == 0
                {
                    return Err(std::io::Error::last_os_error())
                        .context("set embedding named-pipe client nonblocking");
                }
                validate_pipe_dacl(raw(&handle), &current_sid)?;
                let server_pid = named_pipe_server_pid(raw(&handle))?;
                validate_process_sid(server_pid, &current_sid)?;
                let peer_process_start_id = canonical_process_start_identity(server_pid)?;
                let (lifetime_authority_id, listener_id) =
                    windows_authority_ids(&endpoint_namespace_id, server_pid)?;
                return Ok(NativeConnectOutcome::Connected(
                    super::NativeEmbeddingStream {
                        inner: Stream::new(
                            handle,
                            false,
                            TransportIdentity {
                                endpoint_namespace_id,
                                lifetime_authority_id,
                                listener_id,
                                peer_verified: true,
                                peer_pid: Some(server_pid),
                                peer_process_start_id: Some(peer_process_start_id),
                            },
                        )?,
                    },
                ));
            }
            let error = std::io::Error::last_os_error();
            match error.raw_os_error().map(|code| code as u32) {
                Some(ERROR_FILE_NOT_FOUND) => {
                    return Ok(classify_windows_data_pipe_open_error(
                        ERROR_FILE_NOT_FOUND,
                        probe_retained_authority(&authority_pipe_name)?,
                    )
                    .expect("ERROR_FILE_NOT_FOUND is classified"));
                }
                Some(ERROR_PIPE_BUSY) => {
                    let elapsed = Duration::from_nanos(awake_now_ns().saturating_sub(started));
                    let remaining = budget.saturating_sub(elapsed);
                    if remaining.is_zero() {
                        return Ok(NativeConnectOutcome::OwnerUnresponsive);
                    }
                    // WaitNamedPipe uses wall-clock time and can consume
                    // suspend. Short slices leave the unbiased interrupt clock
                    // in charge of the connect budget.
                    let wait_ms = remaining
                        .min(PIPE_CONNECT_POLL)
                        .as_millis()
                        .max(1)
                        .min(u128::from(u32::MAX)) as u32;
                    if unsafe { WaitNamedPipeW(pipe_name.as_ptr(), wait_ms) } == 0 {
                        let wait_error = std::io::Error::last_os_error();
                        if wait_error
                            .raw_os_error()
                            .map(|code| code as u32)
                            .is_some_and(|code| {
                                code == ERROR_SEM_TIMEOUT || code == ERROR_PIPE_BUSY
                            })
                        {
                            continue;
                        }
                        if wait_error.raw_os_error().map(|code| code as u32)
                            == Some(ERROR_FILE_NOT_FOUND)
                        {
                            return Ok(classify_windows_data_pipe_open_error(
                                ERROR_FILE_NOT_FOUND,
                                probe_retained_authority(&authority_pipe_name)?,
                            )
                            .expect("ERROR_FILE_NOT_FOUND is classified"));
                        }
                        return Err(wait_error).context("wait for embedding named pipe");
                    }
                }
                Some(ERROR_ACCESS_DENIED) => {
                    bail!("embedding_endpoint_untrusted: named pipe denied the current account")
                }
                _ => return Err(error).context("connect to embedding named pipe"),
            }
        }
    }

    pub(super) fn bind() -> Result<BindOutcome> {
        let current_sid = current_process_sid()?;
        let sid_string = sid_string(&current_sid)?;
        let (pipe_name, endpoint_namespace_id) = pipe_name(&current_sid)?;
        let authority_pipe_name = authority_pipe_name(&endpoint_namespace_id);
        let security_sddl = wide(&format!(
            "O:{sid_string}D:P(A;;GA;;;{sid_string})(A;;GA;;;SY)"
        ));
        let authority_server =
            match create_pipe_instance(&authority_pipe_name, &security_sddl, true, false) {
                Ok(handle) => handle,
                Err(error)
                    if error
                        .downcast_ref::<std::io::Error>()
                        .and_then(std::io::Error::raw_os_error)
                        .map(|code| code as u32)
                        == Some(ERROR_ACCESS_DENIED) =>
                {
                    return Ok(BindOutcome::AlreadyOwned);
                }
                Err(error) => return Err(error),
            };
        validate_pipe_dacl(raw(&authority_server), &current_sid)?;

        // Retain a private first-instance pipe as the lifetime authority.
        // Product clients know only the separate data-pipe name, so a normal
        // cold connector cannot steal this instance between CreateNamedPipeW
        // and the same-process seal.
        let authority_client_raw = unsafe {
            CreateFileW(
                authority_pipe_name.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                0,
                null(),
                OPEN_EXISTING,
                0,
                null_mut(),
            )
        };
        if authority_client_raw == INVALID_HANDLE_VALUE {
            return Err(std::io::Error::last_os_error())
                .context("connect private embedding authority instance");
        }
        let authority_client = unsafe { OwnedHandle::from_raw_handle(authority_client_raw.cast()) };
        if unsafe { ConnectNamedPipe(raw(&authority_server), null_mut()) } == 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error().map(|code| code as u32) != Some(ERROR_PIPE_CONNECTED) {
                return Err(error).context("seal private embedding authority instance");
            }
        }
        validate_pipe_dacl(raw(&authority_client), &current_sid)?;
        let current_pid = unsafe { GetCurrentProcessId() };
        let client_pid = named_pipe_client_pid(raw(&authority_server))?;
        let server_pid = named_pipe_server_pid(raw(&authority_client))?;
        if client_pid != current_pid || server_pid != current_pid {
            bail!("embedding_windows_first_instance_identity_mismatch");
        }
        let (lifetime_authority_id, listener_id) =
            windows_authority_ids(&endpoint_namespace_id, current_pid)?;
        Ok(BindOutcome::Bound(Listener {
            pipe_name,
            security_sddl,
            current_sid,
            _authority_server: authority_server,
            _authority_client: authority_client,
            identity: TransportIdentity {
                endpoint_namespace_id,
                lifetime_authority_id,
                listener_id,
                peer_verified: true,
                peer_pid: None,
                peer_process_start_id: None,
            },
        }))
    }

    pub(super) fn detach_command(command: &mut Command) {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }

    pub(super) fn clock_api() -> &'static str {
        "QueryUnbiasedInterruptTimePrecise"
    }

    pub(super) fn inclusive_clock_api() -> &'static str {
        "QueryInterruptTimePrecise"
    }

    pub(super) fn inclusive_now_ns() -> Result<u64> {
        use windows_sys::Win32::System::WindowsProgramming::QueryInterruptTimePrecise;

        let mut ticks = 0_u64;
        unsafe { QueryInterruptTimePrecise(&mut ticks) };
        Ok(ticks.saturating_mul(100))
    }

    pub(super) fn boot_id() -> Result<String> {
        let mut now = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        unsafe { GetSystemTimeAsFileTime(&mut now) };
        let now_ticks = (u64::from(now.dwHighDateTime) << 32) | u64::from(now.dwLowDateTime);
        let uptime_ticks = unsafe { GetTickCount64() }.saturating_mul(10_000);
        let boot_ticks = now_ticks.saturating_sub(uptime_ticks);
        // The two kernel reads are not atomic. Round the derived boot instant
        // to ten seconds so independently started clients converge despite
        // their sub-millisecond sampling skew.
        let rounded = boot_ticks.saturating_add(50_000_000) / 100_000_000;
        Ok(format!("windows-filetime-10s:{rounded}"))
    }

    pub(super) fn awake_now_ns() -> u64 {
        let mut ticks = 0_u64;
        unsafe { QueryUnbiasedInterruptTimePrecise(&mut ticks) };
        ticks.saturating_mul(100)
    }

    pub(super) fn clock_resolution_ns() -> Result<u64> {
        Ok(100)
    }

    fn create_pipe_instance(
        pipe_name: &[u16],
        security_sddl: &[u16],
        first: bool,
        nonblocking: bool,
    ) -> Result<OwnedHandle> {
        let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                security_sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                null_mut(),
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error())
                .context("construct embedding named-pipe security descriptor");
        }
        let descriptor = LocalAllocation(descriptor);
        let security = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.0,
            bInheritHandle: 0,
        };
        let open_mode = PIPE_ACCESS_DUPLEX
            | if first {
                FILE_FLAG_FIRST_PIPE_INSTANCE
            } else {
                0
            };
        let pipe_mode = PIPE_TYPE_BYTE
            | PIPE_READMODE_BYTE
            | PIPE_REJECT_REMOTE_CLIENTS
            | if nonblocking { PIPE_NOWAIT } else { PIPE_WAIT };
        let raw_handle = unsafe {
            CreateNamedPipeW(
                pipe_name.as_ptr(),
                open_mode,
                pipe_mode,
                PIPE_UNLIMITED_INSTANCES,
                PIPE_BUFFER_BYTES as u32,
                PIPE_BUFFER_BYTES as u32,
                0,
                &security,
            )
        };
        if raw_handle == INVALID_HANDLE_VALUE {
            return Err(std::io::Error::last_os_error())
                .context("create embedding named-pipe instance");
        }
        Ok(unsafe { OwnedHandle::from_raw_handle(raw_handle.cast()) })
    }

    fn pipe_name(current_sid: &SidBuffer) -> Result<(Vec<u16>, String)> {
        let nonce_salt = qualification_namespace_salt()?;
        let sid = sid_string(current_sid)?;
        let endpoint_namespace_id = sha256_fields(&[
            ENDPOINT_NAMESPACE.as_bytes(),
            b"windows-sid-pipe",
            sid.as_bytes(),
            nonce_salt.as_bytes(),
        ]);
        let name = format!(
            r"\\.\pipe\codestory-embedding-v1-{}",
            &endpoint_namespace_id[..32]
        );
        Ok((wide(&name), endpoint_namespace_id))
    }

    fn authority_pipe_name(endpoint_namespace_id: &str) -> Vec<u16> {
        wide(&format!(
            r"\\.\pipe\codestory-embedding-authority-v1-{}",
            &endpoint_namespace_id[..32]
        ))
    }

    fn probe_retained_authority(
        authority_pipe_name: &[u16],
    ) -> Result<RetainedWindowsAuthorityState> {
        // Observation must never connect to the retained first instance:
        // doing so can steal the authority during the bind-before-seal window.
        if unsafe { WaitNamedPipeW(authority_pipe_name.as_ptr(), 0) } != 0 {
            return Ok(RetainedWindowsAuthorityState::Live);
        }
        let error = std::io::Error::last_os_error();
        match error.raw_os_error().map(|code| code as u32) {
            Some(ERROR_PIPE_BUSY) | Some(ERROR_SEM_TIMEOUT) => {
                Ok(RetainedWindowsAuthorityState::Live)
            }
            Some(ERROR_FILE_NOT_FOUND) => Ok(RetainedWindowsAuthorityState::Absent),
            Some(ERROR_ACCESS_DENIED) => {
                bail!("embedding_lifetime_authority_untrusted: authority pipe denied access")
            }
            _ => Err(error).context("probe retained embedding lifetime authority"),
        }
    }

    fn qualification_namespace_salt() -> Result<String> {
        match (
            std::env::var_os(QUALIFICATION_DIR_ENV),
            std::env::var_os(QUALIFICATION_NONCE_ENV),
        ) {
            (None, None) => Ok("production".into()),
            (Some(dir), Some(nonce)) => {
                let dir = PathBuf::from(dir);
                let nonce = nonce.to_string_lossy();
                if !dir.is_absolute() || !dir.is_dir() {
                    bail!("embedding_qualification_directory_untrusted");
                }
                if nonce.is_empty()
                    || nonce.len() > 64
                    || !nonce
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
                {
                    bail!("embedding_qualification_nonce_invalid");
                }
                Ok(format!("qualification:{nonce}"))
            }
            _ => bail!(
                "embedding_qualification_gate_incomplete: both {QUALIFICATION_DIR_ENV} and {QUALIFICATION_NONCE_ENV} are required"
            ),
        }
    }

    fn windows_authority_ids(
        endpoint_namespace_id: &str,
        server_pid: u32,
    ) -> Result<(String, String)> {
        let start_identity = match codestory_retrieval::probe_process_start_identity(server_pid) {
            codestory_retrieval::ProcessStartProbe::Running { start_identity } => start_identity,
            codestory_retrieval::ProcessStartProbe::NotRunning => {
                bail!("embedding_peer_process_exited")
            }
            codestory_retrieval::ProcessStartProbe::Unknown { reason } => {
                bail!("embedding_peer_process_identity_unavailable: {reason}")
            }
        };
        let authority = sha256_fields(&[
            b"windows-first-pipe-instance-v1",
            endpoint_namespace_id.as_bytes(),
        ]);
        let listener = sha256_fields(&[
            b"windows-named-pipe-listener-v1",
            endpoint_namespace_id.as_bytes(),
            server_pid.to_string().as_bytes(),
            start_identity.as_bytes(),
        ]);
        Ok((authority, listener))
    }

    fn canonical_process_start_identity(pid: u32) -> Result<String> {
        match codestory_retrieval::probe_process_start_identity(pid) {
            codestory_retrieval::ProcessStartProbe::Running { start_identity } => {
                Ok(start_identity)
            }
            codestory_retrieval::ProcessStartProbe::NotRunning => {
                bail!("authenticated Windows peer exited during identity capture")
            }
            codestory_retrieval::ProcessStartProbe::Unknown { reason } => {
                bail!("authenticated Windows peer process identity unavailable: {reason}")
            }
        }
    }

    fn current_process_sid() -> Result<SidBuffer> {
        let process = unsafe { GetCurrentProcess() };
        process_token_sid(process)
    }

    fn validate_process_sid(pid: u32, expected: &SidBuffer) -> Result<()> {
        let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if process.is_null() {
            return Err(std::io::Error::last_os_error())
                .with_context(|| format!("open embedding peer process {pid}"));
        }
        let process = unsafe { OwnedHandle::from_raw_handle(process.cast()) };
        let actual = process_token_sid(raw(&process))?;
        if unsafe { EqualSid(actual.as_psid(), expected.as_psid()) } == 0 {
            bail!("embedding_peer_identity_mismatch: peer token SID differs");
        }
        Ok(())
    }

    fn process_token_sid(process: HANDLE) -> Result<SidBuffer> {
        let mut token = null_mut();
        if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) } == 0 {
            return Err(std::io::Error::last_os_error())
                .context("open embedding peer process token");
        }
        let token = unsafe { OwnedHandle::from_raw_handle(token.cast()) };
        let mut required = 0_u32;
        let _ =
            unsafe { GetTokenInformation(raw(&token), TokenUser, null_mut(), 0, &mut required) };
        let size_error = std::io::Error::last_os_error();
        if size_error.raw_os_error().map(|code| code as u32) != Some(ERROR_INSUFFICIENT_BUFFER)
            || required < std::mem::size_of::<TOKEN_USER>() as u32
        {
            return Err(size_error).context("size embedding peer token identity");
        }
        let mut token_user = vec![0_u8; required as usize];
        if unsafe {
            GetTokenInformation(
                raw(&token),
                TokenUser,
                token_user.as_mut_ptr().cast(),
                required,
                &mut required,
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error())
                .context("read embedding peer token identity");
        }
        let sid = unsafe { (*(token_user.as_ptr().cast::<TOKEN_USER>())).User.Sid };
        SidBuffer::copy_from(sid)
    }

    #[derive(Debug, Clone)]
    struct SidBuffer(Vec<u8>);

    impl SidBuffer {
        fn copy_from(sid: PSID) -> Result<Self> {
            if sid.is_null() {
                bail!("embedding_peer_identity_unavailable: SID was null");
            }
            let length = unsafe { GetLengthSid(sid) };
            if length == 0 {
                return Err(std::io::Error::last_os_error()).context("measure embedding peer SID");
            }
            let mut bytes = vec![0_u8; length as usize];
            if unsafe { CopySid(length, bytes.as_mut_ptr().cast(), sid) } == 0 {
                return Err(std::io::Error::last_os_error()).context("copy embedding peer SID");
            }
            Ok(Self(bytes))
        }

        fn as_psid(&self) -> PSID {
            self.0.as_ptr().cast_mut().cast()
        }
    }

    fn sid_string(sid: &SidBuffer) -> Result<String> {
        let mut string = null_mut();
        if unsafe { ConvertSidToStringSidW(sid.as_psid(), &mut string) } == 0 {
            return Err(std::io::Error::last_os_error()).context("format embedding account SID");
        }
        let allocation = LocalAllocation(string.cast());
        let mut length = 0_usize;
        while unsafe { *string.add(length) } != 0 {
            length += 1;
        }
        let value = String::from_utf16(unsafe { std::slice::from_raw_parts(string, length) })
            .context("decode embedding account SID")?;
        drop(allocation);
        Ok(value)
    }

    fn local_system_sid() -> Result<SidBuffer> {
        let mut required = 0_u32;
        let _ =
            unsafe { CreateWellKnownSid(WinLocalSystemSid, null_mut(), null_mut(), &mut required) };
        let size_error = std::io::Error::last_os_error();
        if size_error.raw_os_error().map(|code| code as u32) != Some(ERROR_INSUFFICIENT_BUFFER) {
            return Err(size_error).context("size LocalSystem SID");
        }
        let mut bytes = vec![0_u8; required as usize];
        if unsafe {
            CreateWellKnownSid(
                WinLocalSystemSid,
                null_mut(),
                bytes.as_mut_ptr().cast(),
                &mut required,
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error()).context("create LocalSystem SID");
        }
        Ok(SidBuffer(bytes))
    }

    fn validate_pipe_dacl(handle: HANDLE, current_sid: &SidBuffer) -> Result<()> {
        let mut owner: PSID = null_mut();
        let mut dacl: *mut ACL = null_mut();
        let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
        let status = unsafe {
            GetSecurityInfo(
                handle,
                SE_KERNEL_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                &mut owner,
                null_mut(),
                &mut dacl,
                null_mut(),
                &mut descriptor,
            )
        };
        if status != 0 {
            return Err(std::io::Error::from_raw_os_error(status as i32))
                .context("read embedding named-pipe DACL");
        }
        let _descriptor = LocalAllocation(descriptor);
        if owner.is_null() || unsafe { EqualSid(owner, current_sid.as_psid()) } == 0 {
            bail!("embedding_endpoint_untrusted: named-pipe owner SID differs");
        }
        if dacl.is_null() {
            bail!("embedding_endpoint_untrusted: named-pipe DACL is absent");
        }
        let mut info = ACL_SIZE_INFORMATION {
            AceCount: 0,
            AclBytesInUse: 0,
            AclBytesFree: 0,
        };
        if unsafe {
            GetAclInformation(
                dacl,
                (&mut info as *mut ACL_SIZE_INFORMATION).cast(),
                std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32,
                AclSizeInformation,
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error())
                .context("inspect embedding named-pipe DACL");
        }
        if info.AceCount != 2 {
            bail!(
                "embedding_endpoint_untrusted: named-pipe DACL has {} entries, expected 2",
                info.AceCount
            );
        }
        let system_sid = local_system_sid()?;
        let mut current_seen = false;
        let mut system_seen = false;
        for index in 0..info.AceCount {
            let mut raw_ace: *mut c_void = null_mut();
            if unsafe { GetAce(dacl, index, &mut raw_ace) } == 0 {
                return Err(std::io::Error::last_os_error())
                    .context("read embedding named-pipe DACL entry");
            }
            let ace = unsafe { &*(raw_ace.cast::<ACCESS_ALLOWED_ACE>()) };
            // ACCESS_ALLOWED_ACE_TYPE is zero. Reject inherited, deny, audit,
            // callback, and object ACEs rather than trying to interpret them.
            if ace.Header.AceType != 0 || (ace.Mask != GENERIC_ALL && ace.Mask != FILE_ALL_ACCESS) {
                bail!(
                    "embedding_endpoint_untrusted: named-pipe DACL entry is not narrow allow-all"
                );
            }
            let ace_sid = (&ace.SidStart as *const u32).cast_mut().cast();
            if unsafe { EqualSid(ace_sid, current_sid.as_psid()) } != 0 {
                if current_seen {
                    bail!("embedding_endpoint_untrusted: duplicate account SID ACE");
                }
                current_seen = true;
            } else if unsafe { EqualSid(ace_sid, system_sid.as_psid()) } != 0 {
                if system_seen {
                    bail!("embedding_endpoint_untrusted: duplicate LocalSystem SID ACE");
                }
                system_seen = true;
            } else {
                bail!("embedding_endpoint_untrusted: named-pipe DACL grants another SID");
            }
        }
        if !current_seen || !system_seen {
            bail!("embedding_endpoint_untrusted: named-pipe DACL is incomplete");
        }
        Ok(())
    }

    fn named_pipe_client_pid(handle: HANDLE) -> Result<u32> {
        let mut pid = 0_u32;
        if unsafe { GetNamedPipeClientProcessId(handle, &mut pid) } == 0 || pid == 0 {
            return Err(std::io::Error::last_os_error())
                .context("read embedding named-pipe client pid");
        }
        Ok(pid)
    }

    fn named_pipe_server_pid(handle: HANDLE) -> Result<u32> {
        let mut pid = 0_u32;
        if unsafe { GetNamedPipeServerProcessId(handle, &mut pid) } == 0 || pid == 0 {
            return Err(std::io::Error::last_os_error())
                .context("read embedding named-pipe server pid");
        }
        Ok(pid)
    }

    fn raw(handle: &OwnedHandle) -> HANDLE {
        handle.as_raw_handle().cast()
    }

    fn timeout_ns(timeout: Option<Duration>) -> u64 {
        timeout
            .map(|timeout| timeout.as_nanos().min(u128::from(NO_TIMEOUT - 1)) as u64)
            .unwrap_or(NO_TIMEOUT)
    }

    fn wait_pipe_io(started_ns: u64, timeout_ns: u64, operation: &str) -> std::io::Result<()> {
        if timeout_ns != NO_TIMEOUT && awake_now_ns().saturating_sub(started_ns) >= timeout_ns {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("embedding named-pipe {operation} timed out"),
            ));
        }
        std::thread::sleep(PIPE_IO_POLL);
        Ok(())
    }

    fn normalize_windows_pipe_io_error(error: std::io::Error) -> std::io::Error {
        if error.raw_os_error().map(|code| code as u32) == Some(ERROR_PIPE_NOT_CONNECTED) {
            // DisconnectNamedPipe reports ERROR_PIPE_NOT_CONNECTED to the
            // remote reader. Rust does not currently assign that Win32 code a
            // portable ErrorKind, so retain the raw error as the source while
            // exposing the precise cross-platform disconnect class.
            return std::io::Error::new(std::io::ErrorKind::NotConnected, error);
        }
        error
    }

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    struct LocalAllocation(*mut c_void);

    impl Drop for LocalAllocation {
        fn drop(&mut self) {
            if !self.0.is_null() {
                let _ = unsafe { LocalFree(self.0) };
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn disconnected_named_pipe_retains_raw_code_and_normalizes_the_kind() {
            let current_sid = current_process_sid().expect("current process SID");
            let sid_string = sid_string(&current_sid).expect("current SID string");
            let security_sddl = wide(&format!(
                "O:{sid_string}D:P(A;;GA;;;{sid_string})(A;;GA;;;SY)"
            ));
            let pipe_name = wide(&format!(
                r"\\.\pipe\codestory-disconnect-test-{}-{}",
                unsafe { GetCurrentProcessId() },
                awake_now_ns()
            ));
            let server = create_pipe_instance(&pipe_name, &security_sddl, true, true)
                .expect("create named-pipe server");
            let client = unsafe {
                CreateFileW(
                    pipe_name.as_ptr(),
                    GENERIC_READ | GENERIC_WRITE,
                    0,
                    null(),
                    OPEN_EXISTING,
                    0,
                    null_mut(),
                )
            };
            assert_ne!(client, INVALID_HANDLE_VALUE, "connect named-pipe client");
            let client = unsafe { OwnedHandle::from_raw_handle(client.cast()) };
            let nonblocking = PIPE_READMODE_BYTE | PIPE_NOWAIT;
            assert_ne!(
                unsafe { SetNamedPipeHandleState(raw(&client), &nonblocking, null(), null()) },
                0,
                "set named-pipe client nonblocking"
            );
            if unsafe { ConnectNamedPipe(raw(&server), null_mut()) } == 0 {
                let error = std::io::Error::last_os_error();
                assert_eq!(
                    error.raw_os_error().map(|code| code as u32),
                    Some(ERROR_PIPE_CONNECTED),
                    "client connection must be the only failed accept state"
                );
            }

            let peer_pid = unsafe { GetCurrentProcessId() };
            let peer_process_start_id =
                canonical_process_start_identity(peer_pid).expect("current process start identity");
            let mut stream = Stream::new(
                client,
                false,
                TransportIdentity {
                    endpoint_namespace_id: "test-endpoint".into(),
                    lifetime_authority_id: "test-authority".into(),
                    listener_id: "test-listener".into(),
                    peer_verified: true,
                    peer_pid: Some(peer_pid),
                    peer_process_start_id: Some(peer_process_start_id),
                },
            )
            .expect("retain named-pipe client stream");
            assert_ne!(
                unsafe { DisconnectNamedPipe(raw(&server)) },
                0,
                "disconnect named-pipe server"
            );

            let error = stream
                .read(&mut [0_u8; 4])
                .expect_err("disconnected named pipe must fail");

            assert_eq!(error.kind(), std::io::ErrorKind::NotConnected);
            assert_eq!(
                error
                    .get_ref()
                    .and_then(|source| source.downcast_ref::<std::io::Error>())
                    .and_then(std::io::Error::raw_os_error)
                    .map(|code| code as u32),
                Some(ERROR_PIPE_NOT_CONNECTED)
            );
            assert!(stream.peer_is_alive().expect("probe retained peer"));
            assert_eq!(stream.peer_exit_code().expect("probe peer exit code"), None);
            eprintln!(
                "named-pipe disconnect evidence: raw_os_error={} normalized_kind={:?} \
                 peer_state=running peer_exit_code=none",
                ERROR_PIPE_NOT_CONNECTED,
                error.kind()
            );
        }

        #[test]
        fn final_response_delivery_waits_for_the_client_to_drain_before_disconnect() {
            let current_sid = current_process_sid().expect("current process SID");
            let sid_string = sid_string(&current_sid).expect("current SID string");
            let security_sddl = wide(&format!(
                "O:{sid_string}D:P(A;;GA;;;{sid_string})(A;;GA;;;SY)"
            ));
            let pipe_name = wide(&format!(
                r"\\.\pipe\codestory-response-delivery-test-{}-{}",
                unsafe { GetCurrentProcessId() },
                awake_now_ns()
            ));
            let server = create_pipe_instance(&pipe_name, &security_sddl, true, true)
                .expect("create named-pipe server");
            let client = unsafe {
                CreateFileW(
                    pipe_name.as_ptr(),
                    GENERIC_READ | GENERIC_WRITE,
                    0,
                    null(),
                    OPEN_EXISTING,
                    0,
                    null_mut(),
                )
            };
            assert_ne!(client, INVALID_HANDLE_VALUE, "connect named-pipe client");
            let client = unsafe { OwnedHandle::from_raw_handle(client.cast()) };
            let nonblocking = PIPE_READMODE_BYTE | PIPE_NOWAIT;
            assert_ne!(
                unsafe { SetNamedPipeHandleState(raw(&client), &nonblocking, null(), null()) },
                0,
                "set named-pipe client nonblocking"
            );
            if unsafe { ConnectNamedPipe(raw(&server), null_mut()) } == 0 {
                let error = std::io::Error::last_os_error();
                assert_eq!(
                    error.raw_os_error().map(|code| code as u32),
                    Some(ERROR_PIPE_CONNECTED),
                    "client connection must be the only failed accept state"
                );
            }

            let peer_pid = unsafe { GetCurrentProcessId() };
            let peer_process_start_id =
                canonical_process_start_identity(peer_pid).expect("current process start identity");
            let identity = || TransportIdentity {
                endpoint_namespace_id: "test-endpoint".into(),
                lifetime_authority_id: "test-authority".into(),
                listener_id: "test-listener".into(),
                peer_verified: true,
                peer_pid: Some(peer_pid),
                peer_process_start_id: Some(peer_process_start_id.clone()),
            };
            let mut server_stream =
                Stream::new(server, true, identity()).expect("retain named-pipe server stream");
            let mut client_stream =
                Stream::new(client, false, identity()).expect("retain named-pipe client stream");
            let timeout = codestory_retrieval::EmbeddingClientBudgets::current().query_request;
            server_stream
                .set_read_timeout(Some(timeout))
                .expect("bound response delivery wait");
            server_stream
                .set_write_timeout(Some(timeout))
                .expect("bound response write");
            client_stream
                .set_read_timeout(Some(timeout))
                .expect("bound response read");

            let expected = (0..(2 * 1024 * 1024))
                .map(|index| (index % 251) as u8)
                .collect::<Vec<_>>();
            let server_expected = expected.clone();
            let (written_tx, written_rx) = std::sync::mpsc::channel();
            let (finished_tx, finished_rx) = std::sync::mpsc::channel();
            let server_thread = std::thread::spawn(move || {
                server_stream
                    .write_all(&server_expected)
                    .expect("write response larger than the pipe buffer");
                written_tx
                    .send(())
                    .expect("signal completed response write");
                server_stream
                    .finish_response_delivery()
                    .expect("wait for client response drain");
                finished_tx
                    .send(())
                    .expect("signal completed response delivery");
            });

            std::thread::sleep(Duration::from_millis(25));
            let mut observed = vec![0_u8; expected.len()];
            client_stream
                .read_exact(&mut observed)
                .expect("read the complete response before server teardown");
            assert_eq!(observed, expected);
            written_rx
                .recv_timeout(timeout)
                .expect("server completed response write");
            assert!(
                finished_rx.try_recv().is_err(),
                "server must retain the pipe while the drained client handle remains open"
            );

            drop(client_stream);
            finished_rx
                .recv_timeout(timeout)
                .expect("client close releases the bounded delivery wait");
            server_thread.join().expect("join named-pipe server");
        }

        #[test]
        fn large_response_write_times_out_when_the_client_does_not_drain() {
            let current_sid = current_process_sid().expect("current process SID");
            let sid_string = sid_string(&current_sid).expect("current SID string");
            let security_sddl = wide(&format!(
                "O:{sid_string}D:P(A;;GA;;;{sid_string})(A;;GA;;;SY)"
            ));
            let pipe_name = wide(&format!(
                r"\\.\pipe\codestory-response-write-timeout-test-{}-{}",
                unsafe { GetCurrentProcessId() },
                awake_now_ns()
            ));
            let server = create_pipe_instance(&pipe_name, &security_sddl, true, true)
                .expect("create named-pipe server");
            let client = unsafe {
                CreateFileW(
                    pipe_name.as_ptr(),
                    GENERIC_READ | GENERIC_WRITE,
                    0,
                    null(),
                    OPEN_EXISTING,
                    0,
                    null_mut(),
                )
            };
            assert_ne!(client, INVALID_HANDLE_VALUE, "connect named-pipe client");
            let client = unsafe { OwnedHandle::from_raw_handle(client.cast()) };
            let nonblocking = PIPE_READMODE_BYTE | PIPE_NOWAIT;
            assert_ne!(
                unsafe { SetNamedPipeHandleState(raw(&client), &nonblocking, null(), null()) },
                0,
                "set named-pipe client nonblocking"
            );
            if unsafe { ConnectNamedPipe(raw(&server), null_mut()) } == 0 {
                let error = std::io::Error::last_os_error();
                assert_eq!(
                    error.raw_os_error().map(|code| code as u32),
                    Some(ERROR_PIPE_CONNECTED),
                    "client connection must be the only failed accept state"
                );
            }

            let peer_pid = unsafe { GetCurrentProcessId() };
            let peer_process_start_id =
                canonical_process_start_identity(peer_pid).expect("current process start identity");
            let identity = TransportIdentity {
                endpoint_namespace_id: "test-endpoint".into(),
                lifetime_authority_id: "test-authority".into(),
                listener_id: "test-listener".into(),
                peer_verified: true,
                peer_pid: Some(peer_pid),
                peer_process_start_id: Some(peer_process_start_id),
            };
            let mut server_stream =
                Stream::new(server, true, identity).expect("retain named-pipe server stream");
            let timeout = Duration::from_millis(50);
            server_stream
                .set_write_timeout(Some(timeout))
                .expect("bound response write");

            let response = vec![7_u8; PIPE_BUFFER_BYTES + 1];
            let started = std::time::Instant::now();
            let error = server_stream
                .write_all(&response)
                .expect_err("a non-reading client must not retain a large response writer");
            assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "the server-owned response write cap must release the handler promptly"
            );

            drop(client);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestAttestationStore {
        endpoint_namespace_id: String,
        content: std::sync::Mutex<Option<Vec<u8>>>,
    }

    impl TestAttestationStore {
        fn new(endpoint_namespace_id: &str) -> Self {
            Self {
                endpoint_namespace_id: endpoint_namespace_id.into(),
                content: std::sync::Mutex::new(None),
            }
        }

        fn replace(&self, content: Option<Vec<u8>>) {
            *self.content.lock().expect("test attestation content") = content;
        }
    }

    impl ExecutableAttestationStore for TestAttestationStore {
        fn endpoint_namespace_id(&self) -> &str {
            &self.endpoint_namespace_id
        }

        fn read(&self) -> Result<Option<Vec<u8>>> {
            Ok(self
                .content
                .lock()
                .expect("test attestation content")
                .clone())
        }

        fn publish(&self, content: &[u8]) -> Result<()> {
            self.replace(Some(content.to_vec()));
            Ok(())
        }
    }

    fn capture_test_file(
        path: &Path,
        mode: ClientTransportMode,
        store: Option<&dyn ExecutableAttestationStore>,
        digest_calls: &mut usize,
    ) -> Result<ExactExecutable> {
        let mut digest = |file: &File, path: &Path| {
            *digest_calls += 1;
            sha256_reader(file, path)
        };
        ExactExecutable::capture_path(path.to_path_buf(), mode, store, &mut digest)
    }

    #[cfg(unix)]
    #[test]
    fn fail_stop_does_not_depend_on_a_live_stderr_reader() {
        use std::os::unix::process::ExitStatusExt;

        const CHILD_ENV: &str = "CODESTORY_TEST_FAIL_STOP_WITH_CLOSED_STDERR";
        if std::env::var_os(CHILD_ENV).is_some() {
            unsafe {
                libc::close(libc::STDERR_FILENO);
            }
            fail_stop_process("embedding_engine_stalled");
        }

        let status = Command::new(std::env::current_exe().expect("current test executable"))
            .arg("--exact")
            .arg("embedding_server_transport::tests::fail_stop_does_not_depend_on_a_live_stderr_reader")
            .arg("--nocapture")
            .env(CHILD_ENV, "1")
            .status()
            .expect("run fail-stop child");

        assert_eq!(status.signal(), Some(libc::SIGABRT));
    }

    #[test]
    fn sha256_validation_is_exact() {
        assert!(is_sha256(&"a".repeat(64)));
        assert!(is_sha256(&"A".repeat(64)));
        assert!(!is_sha256(&"a".repeat(63)));
        assert!(!is_sha256(&format!("{}g", "a".repeat(63))));
    }

    #[test]
    fn structured_hash_separates_fields() {
        assert_ne!(sha256_fields(&[b"ab", b"c"]), sha256_fields(&[b"a", b"bc"]));
    }

    #[test]
    fn observe_capture_reuses_only_an_exact_matching_attestation() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let executable_path = directory.path().join("codestory-cli");
        std::fs::write(&executable_path, b"exact candidate bytes")?;
        let store = TestAttestationStore::new("normal-authority");

        let mut fresh_calls = 0;
        let fresh = capture_test_file(
            &executable_path,
            ClientTransportMode::SpawnCapable,
            None,
            &mut fresh_calls,
        )?;
        assert_eq!(fresh_calls, 1);
        publish_attestation(&store, &fresh)?;

        let mut warm_calls = 0;
        let warm = capture_test_file(
            &executable_path,
            ClientTransportMode::ObserveOnly,
            Some(&store),
            &mut warm_calls,
        )?;
        assert_eq!(
            warm_calls, 0,
            "a native-identity match should reuse the digest"
        );
        assert_eq!(warm.sha256(), fresh.sha256());

        let mut substituted = serde_json::from_slice::<ExecutableDigestAttestation>(
            &store.read()?.expect("published attestation"),
        )?;
        substituted.executable_sha256 = "f".repeat(64);
        store.replace(Some(serde_json::to_vec(&substituted)?));
        let mut substituted_calls = 0;
        capture_test_file(
            &executable_path,
            ClientTransportMode::ObserveOnly,
            Some(&store),
            &mut substituted_calls,
        )?;
        assert_eq!(
            substituted_calls, 1,
            "a valid-shape digest substitution without a matching record checksum must fresh-hash"
        );

        store.replace(Some(br#"{"schema_version":1,"tampered":true}"#.to_vec()));
        let mut tampered_calls = 0;
        capture_test_file(
            &executable_path,
            ClientTransportMode::ObserveOnly,
            Some(&store),
            &mut tampered_calls,
        )?;
        assert_eq!(tampered_calls, 1, "tampered evidence must fresh-hash");

        store.replace(None);
        let mut missing_calls = 0;
        capture_test_file(
            &executable_path,
            ClientTransportMode::ObserveOnly,
            Some(&store),
            &mut missing_calls,
        )?;
        assert_eq!(missing_calls, 1, "missing evidence must fresh-hash");
        Ok(())
    }

    #[test]
    fn observe_capture_invalidates_attestation_after_file_identity_change() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let executable_path = directory.path().join("codestory-cli");
        std::fs::write(&executable_path, b"first exact candidate")?;
        let store = TestAttestationStore::new("normal-authority");
        let mut fresh_calls = 0;
        let fresh = capture_test_file(
            &executable_path,
            ClientTransportMode::SpawnCapable,
            None,
            &mut fresh_calls,
        )?;
        publish_attestation(&store, &fresh)?;

        std::fs::write(
            &executable_path,
            b"changed exact candidate with another size",
        )?;
        let mut changed_calls = 0;
        let changed = capture_test_file(
            &executable_path,
            ClientTransportMode::ObserveOnly,
            Some(&store),
            &mut changed_calls,
        )?;

        assert_eq!(changed_calls, 1);
        assert_ne!(changed.sha256(), fresh.sha256());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn observe_capture_invalidates_same_size_rewrite_with_restored_mtime() -> Result<()> {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::fs::MetadataExt;

        let directory = tempfile::tempdir()?;
        let executable_path = directory.path().join("codestory-cli");
        let original = b"first exact candidate";
        let changed = b"other exact candidate";
        assert_eq!(original.len(), changed.len());
        std::fs::write(&executable_path, original)?;
        let original_metadata = std::fs::metadata(&executable_path)?;
        let store = TestAttestationStore::new("normal-authority");
        let mut fresh_calls = 0;
        let fresh = capture_test_file(
            &executable_path,
            ClientTransportMode::SpawnCapable,
            None,
            &mut fresh_calls,
        )?;
        publish_attestation(&store, &fresh)?;

        std::thread::sleep(Duration::from_millis(2));
        std::fs::write(&executable_path, changed)?;
        let path = CString::new(executable_path.as_os_str().as_bytes())?;
        let times = [
            libc::timespec {
                tv_sec: original_metadata.atime(),
                tv_nsec: original_metadata.atime_nsec(),
            },
            libc::timespec {
                tv_sec: original_metadata.mtime(),
                tv_nsec: original_metadata.mtime_nsec(),
            },
        ];
        if unsafe { libc::utimensat(libc::AT_FDCWD, path.as_ptr(), times.as_ptr(), 0) } != 0 {
            return Err(std::io::Error::last_os_error()).context("restore test executable mtime");
        }
        let changed_metadata = std::fs::metadata(&executable_path)?;
        assert_eq!(changed_metadata.len(), original_metadata.len());
        assert_eq!(changed_metadata.mtime(), original_metadata.mtime());
        assert_eq!(
            changed_metadata.mtime_nsec(),
            original_metadata.mtime_nsec()
        );
        assert_ne!(
            (changed_metadata.ctime(), changed_metadata.ctime_nsec()),
            (original_metadata.ctime(), original_metadata.ctime_nsec()),
            "the test requires Unix change-time evidence"
        );

        let mut changed_calls = 0;
        let observed = capture_test_file(
            &executable_path,
            ClientTransportMode::ObserveOnly,
            Some(&store),
            &mut changed_calls,
        )?;
        assert_eq!(changed_calls, 1, "ctime drift must force a fresh hash");
        assert_ne!(observed.sha256(), fresh.sha256());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn native_attestation_store_is_private_atomic_and_namespace_isolated() -> Result<()> {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        let directory = tempfile::tempdir()?;
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))?;
        let normal = platform::test_executable_attestation_store(directory.path(), "normal")?;
        let qualification =
            platform::test_executable_attestation_store(directory.path(), "qualification")?;

        normal.publish(b"first")?;
        normal.publish(b"second")?;
        assert_eq!(normal.read()?.as_deref(), Some(b"second".as_slice()));
        assert_eq!(qualification.read()?, None);
        assert_ne!(normal.test_path(), qualification.test_path());
        let metadata = std::fs::symlink_metadata(normal.test_path())?;
        assert!(metadata.is_file());
        assert_eq!(metadata.uid(), unsafe { libc::geteuid() });
        assert_eq!(metadata.mode() & 0o077, 0);
        assert_eq!(metadata.nlink(), 1);
        assert!(
            std::fs::read_dir(directory.path())?.all(|entry| !entry
                .expect("attestation entry")
                .file_name()
                .to_string_lossy()
                .ends_with(".tmp")),
            "atomic publication must not leave temporary evidence"
        );

        std::fs::set_permissions(normal.test_path(), std::fs::Permissions::from_mode(0o644))?;
        let error = normal
            .read()
            .expect_err("broadly readable attestation must be rejected");
        assert!(format!("{error:#}").contains("embedding_executable_attestation_untrusted"));
        std::fs::set_permissions(normal.test_path(), std::fs::Permissions::from_mode(0o600))?;
        std::fs::hard_link(
            normal.test_path(),
            directory.path().join("attestation-link"),
        )?;
        let error = normal
            .read()
            .expect_err("multiply linked attestation must be rejected");
        assert!(format!("{error:#}").contains("embedding_executable_attestation_untrusted"));
        Ok(())
    }

    #[test]
    fn observe_only_transport_cannot_authorize_server_spawn() -> Result<()> {
        let transport =
            NativeEmbeddingClientTransport::capture_with_mode(ClientTransportMode::ObserveOnly)?;
        let error = transport
            .spawn_exact_current_exe()
            .expect_err("cached observe evidence must never authorize spawn");
        assert!(format!("{error:#}").contains("embedding_server_spawn_forbidden"));
        Ok(())
    }

    #[test]
    fn connect_budget_covers_setup_and_peer_authentication() {
        let budget = Duration::from_millis(20);
        assert_eq!(
            remaining_awake_budget(100, 10_000_100, budget),
            Some(Duration::from_millis(10))
        );
        assert_eq!(remaining_awake_budget(100, 20_000_100, budget), None);
        assert_eq!(remaining_awake_budget(100, 30_000_100, budget), None);
    }

    #[test]
    fn embedding_server_spawn_failures_are_attempt_scoped_and_repeatable() {
        let delayed_attempt = codestory_retrieval::EmbeddingSpawnAttempt::new(1);
        let delayed_reaper = delayed_attempt.clone();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let reaper = std::thread::spawn(move || {
            ready_tx.send(()).expect("signal delayed reaper");
            release_rx.recv().expect("release delayed reaper");
            delayed_reaper.record_failure(codestory_retrieval::EmbeddingTransportFailure {
                code: "embedding_server_start_failed".into(),
                message: "delayed prior failure".into(),
            });
        });
        ready_rx.recv().expect("wait for delayed attempt");

        let viable_retry = codestory_retrieval::EmbeddingSpawnAttempt::new(2);
        viable_retry.record_success();
        release_tx.send(()).expect("release delayed failure");
        reaper.join().expect("join delayed reaper");
        assert!(
            viable_retry.failure().is_none(),
            "an earlier reaper cannot poison a viable retry generation"
        );
        assert_eq!(
            delayed_attempt
                .failure()
                .expect("the delayed reaper only completes its own attempt")
                .message,
            "delayed prior failure"
        );

        let current_attempt = codestory_retrieval::EmbeddingSpawnAttempt::new(3);
        current_attempt.record_failure(codestory_retrieval::EmbeddingTransportFailure {
            code: "embedding_server_start_failed".into(),
            message: "current bind failed".into(),
        });
        assert_eq!(
            current_attempt
                .failure()
                .expect("first waiter observes the current failure")
                .message,
            "current bind failed"
        );
        assert_eq!(
            current_attempt
                .failure()
                .expect("second waiter observes the same current failure")
                .message,
            "current bind failed"
        );
    }

    #[test]
    fn missing_windows_data_pipe_with_live_authority_is_owner_unresponsive() {
        let outcome = classify_windows_data_pipe_open_error(
            WINDOWS_ERROR_FILE_NOT_FOUND_CODE,
            RetainedWindowsAuthorityState::Live,
        )
        .expect("missing data pipe is classified");
        assert!(matches!(outcome, NativeConnectOutcome::OwnerUnresponsive));

        let outcome = classify_windows_data_pipe_open_error(
            WINDOWS_ERROR_FILE_NOT_FOUND_CODE,
            RetainedWindowsAuthorityState::Absent,
        )
        .expect("missing data pipe is classified");
        assert!(matches!(outcome, NativeConnectOutcome::NoOwner));
    }

    #[test]
    fn captured_executable_identity_includes_content_metadata() {
        let executable = ExactExecutable::capture().expect("capture exact executable");
        let file = File::open(executable.path()).expect("open exact executable");
        let identity =
            executable_file_identity(&file).expect("capture executable metadata identity");
        assert_eq!(identity, executable.file_identity);
        assert!(identity.file_size > 0);
    }
}
