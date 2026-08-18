//! Process-local failure evidence for the CLI and embedded server.
//!
//! The sink is intentionally independent of project activation. It writes only
//! when WARN+ evidence exists, so observational commands do not create cache
//! state merely by starting. Panic and fail-stop paths use the same bounded,
//! private sink and carry a process correlation ID shared with the plugin
//! launcher.

use anyhow::{Context, Result, bail};
use codestory_contracts::bounded_locks::{self, FileLockKind};
use serde_json::{Map, Value, json};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock, mpsc};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::layer::{Context as LayerContext, SubscriberExt};
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{Layer, Registry};
use uuid::Uuid;

const LOG_ENV: &str = "CODESTORY_LOG";
const CORRELATION_ENV: &str = "CODESTORY_LOG_CORRELATION_ID";
const DIAGNOSTICS_DIR: &str = "diagnostics";
const LOG_FILE: &str = "codestory.jsonl";
const LOCK_FILE: &str = ".codestory-log.lock";
const DEFAULT_LOG_BYTES: u64 = 2 * 1024 * 1024;
const RETAINED_LOGS: usize = 3;
const MAX_FIELD_BYTES: usize = 4096;
const MAX_RECORD_BYTES: usize = 64 * 1024;
const MAX_ERROR_CHAIN_COUNT: usize = 64;
const EMERGENCY_LOG_SLOTS: usize = 16;
const FAIL_STOP_MARKER_SLOTS: usize = 16;
const FAIL_STOP_MARKER_TIMEOUT: Duration = Duration::from_millis(250);
const REDACTED: &str = "[redacted]";
const PANIC_STDERR_NOTICE: &[u8] =
    b"CodeStory recorded a panic diagnostic; payload and free-form text were redacted.\n";

static PROCESS_DIAGNOSTICS: OnceLock<Arc<DiagnosticSink>> = OnceLock::new();
static PANIC_HOOK_INSTALLED: OnceLock<()> = OnceLock::new();
static EMERGENCY_SLOT_COUNTER: AtomicUsize = AtomicUsize::new(0);
static FAIL_STOP_SLOT_COUNTER: AtomicUsize = AtomicUsize::new(0);

pub(crate) fn install_process_diagnostics() {
    let sink = process_diagnostics();
    if let Some(level) = configured_level(std::env::var(LOG_ENV).ok().as_deref()) {
        let layer = DiagnosticLayer::new(Arc::clone(&sink)).with_filter(level);
        // An embedding application may already own the global subscriber. The
        // panic/fail-stop writer remains available even when that is the case.
        let _ = Registry::default().with(layer).try_init();
    }
    install_panic_hook();
    install_activation_fail_stop_hook();
}

/// A cancelled activation worker that never reaches a quiescent boundary may
/// still hold a publication or store lock. Detaching it and serving on would
/// let owned mutation continue behind an evicted context, so the hosting
/// process records evidence and stops.
fn install_activation_fail_stop_hook() {
    codestory_runtime::set_activation_fail_stop_hook(Some(Arc::new(|reason_code: &str| {
        record_fail_stop(reason_code);
        std::process::abort();
    })));
}

pub(crate) fn record_command_failure(error: &anyhow::Error) {
    let record = command_failure_record(error);
    let _ = process_diagnostics().write_record(record);
}

fn command_failure_record(error: &anyhow::Error) -> Value {
    let chain_count = error.chain().take(MAX_ERROR_CHAIN_COUNT + 1).count();
    json!({
        "event": "command_failure",
        "level": "ERROR",
        "error": REDACTED,
        "error_chain_count": chain_count.min(MAX_ERROR_CHAIN_COUNT),
        "error_chain_count_capped": chain_count > MAX_ERROR_CHAIN_COUNT,
    })
}

pub(crate) fn record_fail_stop(reason_code: &str) {
    let reason_code = safe_reason_code(reason_code);
    let _ = run_bounded_attempt(FAIL_STOP_MARKER_TIMEOUT, move || {
        let sink = process_diagnostics();
        let record = sink.decorate(json!({
            "event": "process_fail_stop",
            "level": "ERROR",
            "reason_code": reason_code,
        }));
        // Fail-stop evidence never waits for or appends to the rotating log.
        // The caller aborts after the fixed outer deadline even when this
        // best-effort marker attempt is stalled in the filesystem.
        let _ = sink.write_fail_stop_marker(&record);
    });
}

fn run_bounded_attempt<F>(timeout: Duration, attempt: F) -> bool
where
    F: FnOnce() + Send + 'static,
{
    let (completed_tx, completed_rx) = mpsc::sync_channel(1);
    let spawned = thread::Builder::new()
        .name("codestory-fail-stop-evidence".into())
        .spawn(move || {
            attempt();
            let _ = completed_tx.try_send(());
        });
    if spawned.is_err() {
        return false;
    }
    completed_rx.recv_timeout(timeout).is_ok()
}

fn process_diagnostics() -> Arc<DiagnosticSink> {
    Arc::clone(PROCESS_DIAGNOSTICS.get_or_init(|| {
        let defaults = crate::sidecar_runtime::process_defaults();
        Arc::new(DiagnosticSink::new(
            defaults.cache_root().to_path_buf(),
            process_correlation_id(std::env::var(CORRELATION_ENV).ok().as_deref()),
            DEFAULT_LOG_BYTES,
        ))
    }))
}

fn install_panic_hook() {
    PANIC_HOOK_INSTALLED.get_or_init(|| {
        std::panic::set_hook(Box::new(move |info| {
            let location = info.location().map(|location| {
                format!(
                    "{}:{}:{}",
                    location.file(),
                    location.line(),
                    location.column()
                )
            });
            let mut stderr = std::io::stderr().lock();
            record_panic_evidence(
                process_diagnostics().as_ref(),
                info.payload(),
                location,
                &mut stderr,
            );
        }));
    });
}

fn record_panic_evidence(
    sink: &DiagnosticSink,
    payload: &(dyn std::any::Any + Send),
    location: Option<String>,
    stderr: &mut impl Write,
) {
    let _ = sink.write_record(panic_record(payload, location));
    write_safe_panic_notice(stderr);
}

fn write_safe_panic_notice(writer: &mut impl Write) {
    let _ = writer.write_all(PANIC_STDERR_NOTICE);
    let _ = writer.flush();
}

fn panic_record(payload: &(dyn std::any::Any + Send), location: Option<String>) -> Value {
    let (payload_kind, payload_bytes) = if let Some(message) = payload.downcast_ref::<&str>() {
        ("str", Some(message.len()))
    } else if let Some(message) = payload.downcast_ref::<String>() {
        ("string", Some(message.len()))
    } else {
        ("non_string", None)
    };
    json!({
        "event": "panic",
        "level": "ERROR",
        "payload": REDACTED,
        "payload_kind": payload_kind,
        "payload_bytes": payload_bytes,
        "location": location.map(|value| bounded_text(&value)),
    })
}

#[derive(Debug)]
struct DiagnosticSink {
    cache_root: PathBuf,
    correlation_id: String,
    max_log_bytes: u64,
}

impl DiagnosticSink {
    fn new(cache_root: PathBuf, correlation_id: String, max_log_bytes: u64) -> Self {
        Self {
            cache_root,
            correlation_id,
            max_log_bytes,
        }
    }

    fn diagnostics_dir(&self) -> PathBuf {
        self.cache_root.join(DIAGNOSTICS_DIR)
    }

    fn log_path(&self) -> PathBuf {
        self.diagnostics_dir().join(LOG_FILE)
    }

    fn decorate(&self, mut record: Value) -> Value {
        if !record.is_object() {
            record = json!({"event": "invalid_diagnostic_record"});
        }
        let object = record
            .as_object_mut()
            .expect("diagnostic record is an object");
        object.insert("schema_version".into(), Value::from(1));
        object.insert("timestamp_unix_ms".into(), Value::from(unix_timestamp_ms()));
        object.insert("pid".into(), Value::from(std::process::id()));
        object.insert(
            "correlation_id".into(),
            Value::from(self.correlation_id.clone()),
        );
        record
    }

    fn write_record(&self, record: Value) -> Result<()> {
        self.write_decorated_record(self.decorate(record))
    }

    fn write_decorated_record(&self, record: Value) -> Result<()> {
        let mut encoded = serde_json::to_vec(&record).context("encode diagnostic record")?;
        if encoded.len() > MAX_RECORD_BYTES {
            encoded = serde_json::to_vec(&self.decorate(json!({
                "event": "diagnostic_record_truncated",
                "level": "ERROR",
                "original_bytes": encoded.len(),
            })))
            .context("encode truncated diagnostic record")?;
        }
        encoded.push(b'\n');
        self.append_line(&encoded)
    }

    fn append_line(&self, encoded: &[u8]) -> Result<()> {
        let directory = self.diagnostics_dir();
        ensure_private_directory(&directory)?;
        let lock_path = directory.join(LOCK_FILE);
        let lock = open_private_file(&lock_path, false, false)?;
        if !bounded_locks::try_acquire(&lock, FileLockKind::Exclusive)
            .context("lock diagnostic log")?
        {
            return self.append_emergency_line(encoded);
        }
        let result = self.append_locked_line(encoded);
        let _ = bounded_locks::release(&lock);
        result
    }

    fn append_locked_line(&self, encoded: &[u8]) -> Result<()> {
        let path = self.log_path();
        refuse_symlink(&path)?;
        let current_bytes = fs::metadata(&path)
            .map(|metadata| metadata.len())
            .unwrap_or(0);
        if current_bytes > 0
            && current_bytes.saturating_add(encoded.len() as u64) > self.max_log_bytes
        {
            rotate_logs(&path)?;
        }
        let mut file = open_private_file(&path, true, false)?;
        file.write_all(encoded).context("write diagnostic log")?;
        file.flush().context("flush diagnostic log")?;
        file.sync_data().context("sync diagnostic log")?;
        enforce_private_file_mode(&path)?;
        if let Some(directory) = path.parent() {
            sync_directory(directory)?;
        }
        Ok(())
    }

    fn append_emergency_line(&self, encoded: &[u8]) -> Result<()> {
        let slot = next_bounded_slot(&EMERGENCY_SLOT_COUNTER, EMERGENCY_LOG_SLOTS);
        write_bounded_slot(&self.diagnostics_dir(), "emergency", "jsonl", slot, encoded).map(|_| ())
    }

    fn write_fail_stop_marker(&self, record: &Value) -> Result<PathBuf> {
        let directory = self.diagnostics_dir();
        ensure_private_directory(&directory)?;
        let mut encoded = serde_json::to_vec(record).context("encode fail-stop marker")?;
        encoded.push(b'\n');
        let slot = next_bounded_slot(&FAIL_STOP_SLOT_COUNTER, FAIL_STOP_MARKER_SLOTS);
        write_bounded_slot(&directory, "fail-stop", "json", slot, &encoded)
    }
}

fn next_bounded_slot(counter: &AtomicUsize, slots: usize) -> usize {
    (std::process::id() as usize).wrapping_add(counter.fetch_add(1, Ordering::Relaxed)) % slots
}

fn write_bounded_slot(
    directory: &Path,
    stem: &str,
    extension: &str,
    slot: usize,
    encoded: &[u8],
) -> Result<PathBuf> {
    ensure_private_directory(directory)?;
    let destination = directory.join(format!("{stem}-{slot:02}.{extension}"));
    let temporary = directory.join(format!(".{stem}-{slot:02}.tmp"));
    let slot_lock_path = directory.join(format!(".{stem}-{slot:02}.lock"));
    refuse_symlink(&slot_lock_path)?;
    let slot_lock = open_private_file(&slot_lock_path, false, false)?;
    if !bounded_locks::try_acquire(&slot_lock, FileLockKind::Exclusive)
        .with_context(|| format!("lock bounded {stem} evidence slot"))?
    {
        bail!("bounded {stem} evidence slot is busy");
    }
    let result = (|| -> Result<PathBuf> {
        refuse_symlink(&temporary)?;
        let mut file = open_private_truncated_file(&temporary)?;
        file.write_all(encoded)
            .with_context(|| format!("write bounded {stem} evidence"))?;
        file.flush()
            .with_context(|| format!("flush bounded {stem} evidence"))?;
        file.sync_all()
            .with_context(|| format!("sync bounded {stem} evidence"))?;
        drop(file);
        publish_bounded_slot(&temporary, &destination, stem)?;
        enforce_private_file_mode(&destination)?;
        sync_directory(directory)?;
        Ok(destination)
    })();
    let _ = bounded_locks::release(&slot_lock);
    result
}

fn publish_bounded_slot(temporary: &Path, destination: &Path, stem: &str) -> Result<()> {
    match fs::rename(temporary, destination) {
        Ok(()) => Ok(()),
        Err(error) => {
            #[cfg(windows)]
            {
                let _ = error;
                refuse_symlink(destination)?;
                remove_file_if_present(destination)?;
                fs::rename(temporary, destination)
                    .with_context(|| format!("publish bounded {stem} evidence"))?;
                Ok(())
            }
            #[cfg(not(windows))]
            {
                Err(error).with_context(|| format!("publish bounded {stem} evidence"))
            }
        }
    }
}

#[derive(Clone)]
struct DiagnosticLayer {
    sink: Arc<DiagnosticSink>,
}

impl DiagnosticLayer {
    fn new(sink: Arc<DiagnosticSink>) -> Self {
        Self { sink }
    }
}

impl<S> Layer<S> for DiagnosticLayer
where
    S: Subscriber,
{
    fn on_event(&self, event: &Event<'_>, _context: LayerContext<'_, S>) {
        let metadata = event.metadata();
        let mut visitor = DiagnosticVisitor::default();
        event.record(&mut visitor);
        let record = json!({
            "event": "tracing_event",
            "level": metadata.level().as_str(),
            "target": safe_diagnostic_token(metadata.target()).unwrap_or(REDACTED),
            "code_file": metadata.file().map(bounded_text),
            "code_line": metadata.line(),
            "fields": visitor.fields,
        });
        let _ = self.sink.write_record(record);
    }
}

#[derive(Default)]
struct DiagnosticVisitor {
    fields: Map<String, Value>,
}

impl DiagnosticVisitor {
    fn insert_typed(&mut self, field: &Field, value: Value) {
        let name = field.name();
        if safe_diagnostic_token(name).is_none() {
            return;
        }
        self.fields.insert(name.to_string(), value);
    }

    fn insert_redacted(&mut self, field: &Field) {
        self.insert_typed(field, Value::from(REDACTED));
    }
}

impl Visit for DiagnosticVisitor {
    fn record_bool(&mut self, field: &Field, value: bool) {
        self.insert_typed(field, Value::from(value));
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.insert_typed(field, Value::from(value));
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.insert_typed(field, Value::from(value));
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        self.insert_typed(field, Value::from(value));
    }

    fn record_str(&mut self, field: &Field, _value: &str) {
        self.insert_redacted(field);
    }

    fn record_error(&mut self, field: &Field, _value: &(dyn std::error::Error + 'static)) {
        self.insert_redacted(field);
    }

    fn record_debug(&mut self, field: &Field, _value: &dyn std::fmt::Debug) {
        self.insert_redacted(field);
    }
}

fn configured_level(raw: Option<&str>) -> Option<LevelFilter> {
    match raw.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
        Some("off" | "0" | "false") => None,
        Some("error") => Some(LevelFilter::ERROR),
        // File diagnostics deliberately never drop below WARN: existing lower
        // level events may contain product inputs that predate this sink.
        _ => Some(LevelFilter::WARN),
    }
}

fn process_correlation_id(raw: Option<&str>) -> String {
    raw.filter(|value| {
        !value.is_empty()
            && value.len() <= 128
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    })
    .map(str::to_string)
    .unwrap_or_else(|| Uuid::new_v4().simple().to_string())
}

fn safe_diagnostic_token(value: &str) -> Option<&str> {
    (!value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b':')))
    .then_some(value)
}

fn bounded_text(value: &str) -> String {
    if value.len() <= MAX_FIELD_BYTES {
        return value.to_string();
    }
    let mut end = MAX_FIELD_BYTES;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...[truncated:{} bytes]", &value[..end], value.len())
}

fn safe_reason_code(value: &str) -> String {
    match value {
        "embedding_engine_stalled"
        | "embedding_qualification_crash"
        | codestory_runtime::ACTIVATION_QUIESCENCE_FAIL_STOP => value.to_string(),
        _ => "unknown_fail_stop".into(),
    }
}

fn unix_timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

fn ensure_private_directory(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            bail!("diagnostic directory is a symlink: {}", path.display())
        }
        Ok(metadata) if !metadata.is_dir() => {
            bail!("diagnostic path is not a directory: {}", path.display())
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(path).context("create diagnostic directory")?;
        }
        Err(error) => return Err(error).context("inspect diagnostic directory"),
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .context("set diagnostic directory permissions")?;
    }
    Ok(())
}

fn refuse_symlink(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            bail!("diagnostic file is a symlink: {}", path.display())
        }
        Ok(metadata) if !metadata.is_file() => {
            bail!("diagnostic path is not a file: {}", path.display())
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).context("inspect diagnostic file"),
    }
}

fn open_private_file(path: &Path, append: bool, create_new: bool) -> Result<File> {
    let mut options = OpenOptions::new();
    options
        .read(!append)
        .write(true)
        .append(append)
        .create(!create_new)
        .create_new(create_new);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let file = options
        .open(path)
        .with_context(|| format!("open private diagnostic file {}", path.display()))?;
    enforce_private_file_mode(path)?;
    Ok(file)
}

fn open_private_truncated_file(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let file = options
        .open(path)
        .with_context(|| format!("open private bounded evidence file {}", path.display()))?;
    enforce_private_file_mode(path)?;
    Ok(file)
}

fn enforce_private_file_mode(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .with_context(|| format!("set private mode on {}", path.display()))?;
    }
    Ok(())
}

fn rotate_logs(path: &Path) -> Result<()> {
    for index in (1..=RETAINED_LOGS).rev() {
        let destination = rotated_path(path, index);
        if index == RETAINED_LOGS {
            remove_file_if_present(&destination)?;
        }
        let source = if index == 1 {
            path.to_path_buf()
        } else {
            rotated_path(path, index - 1)
        };
        if source.exists() {
            remove_file_if_present(&destination)?;
            fs::rename(&source, &destination).with_context(|| {
                format!(
                    "rotate diagnostic log {} to {}",
                    source.display(),
                    destination.display()
                )
            })?;
        }
    }
    if let Some(directory) = path.parent() {
        sync_directory(directory)?;
    }
    Ok(())
}

fn rotated_path(path: &Path, index: usize) -> PathBuf {
    path.with_file_name(format!("{LOG_FILE}.{index}"))
}

fn remove_file_if_present(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("remove {}", path.display())),
    }
}

fn sync_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .context("sync diagnostic directory")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn configured_log_level_never_drops_below_warn() {
        assert_eq!(configured_level(None), Some(LevelFilter::WARN));
        assert_eq!(configured_level(Some("error")), Some(LevelFilter::ERROR));
        assert_eq!(configured_level(Some("off")), None);
        assert_eq!(configured_level(Some("debug")), Some(LevelFilter::WARN));
    }

    #[test]
    fn tracing_sink_redacts_every_unclassified_string() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let sink = Arc::new(DiagnosticSink::new(
            directory.path().to_path_buf(),
            "test-correlation".into(),
            DEFAULT_LOG_BYTES,
        ));
        let subscriber = Registry::default()
            .with(DiagnosticLayer::new(Arc::clone(&sink)).with_filter(LevelFilter::WARN));
        tracing::subscriber::with_default(subscriber, || {
            tracing::warn!(
                query = "private query",
                source = "private source",
                safe_code = "embedding_server_dead",
                numeric_code = 17_u64,
                retryable = true,
                "unlabeled private query"
            );
            tracing::info!("not retained");
        });

        let rows = read_jsonl(&sink.log_path())?;
        assert_eq!(rows.len(), 1);
        let encoded = serde_json::to_string(&rows[0])?;
        assert!(!encoded.contains("unlabeled private query"));
        assert!(!encoded.contains("private source"));
        assert!(!encoded.contains("embedding_server_dead"));
        assert_eq!(rows[0]["fields"]["message"], REDACTED);
        assert_eq!(rows[0]["fields"]["query"], "[redacted]");
        assert_eq!(rows[0]["fields"]["source"], "[redacted]");
        assert_eq!(rows[0]["fields"]["safe_code"], REDACTED);
        assert_eq!(rows[0]["fields"]["numeric_code"], 17);
        assert_eq!(rows[0]["fields"]["retryable"], true);
        assert_eq!(rows[0]["correlation_id"], "test-correlation");
        Ok(())
    }

    #[test]
    fn command_failure_drops_unlabeled_private_text_without_a_digest() -> Result<()> {
        let record = command_failure_record(&anyhow::anyhow!("unlabeled private query"));
        let encoded = serde_json::to_string(&record)?;
        assert!(!encoded.contains("unlabeled private query"));
        assert_eq!(record["error"], REDACTED);
        assert_eq!(record["error_chain_count"], 1);
        assert!(record.get("error_sha256").is_none());
        Ok(())
    }

    #[test]
    fn oversized_record_drops_unclassified_text_without_a_digest() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let sink = DiagnosticSink::new(
            directory.path().to_path_buf(),
            "oversized-test".into(),
            DEFAULT_LOG_BYTES,
        );
        let private_text = format!("unlabeled-private-query-{}", "x".repeat(MAX_RECORD_BYTES));
        sink.write_record(json!({
            "event": "test_only_oversized_record",
            "message": private_text,
        }))?;

        let record = read_jsonl(&sink.log_path())?.remove(0);
        let encoded = serde_json::to_string(&record)?;
        assert!(!encoded.contains("unlabeled-private-query"));
        assert_eq!(record["event"], "diagnostic_record_truncated");
        assert!(record["original_bytes"].as_u64().is_some());
        assert!(record.get("record_sha256").is_none());
        Ok(())
    }

    #[test]
    fn panic_record_drops_unlabeled_payload_and_backtrace() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let sink = DiagnosticSink::new(
            directory.path().to_path_buf(),
            "panic-test".into(),
            DEFAULT_LOG_BYTES,
        );
        let payload = "unlabeled private query".to_string();
        let mut captured_stderr = Vec::new();
        record_panic_evidence(
            &sink,
            &payload,
            Some("src/runtime.rs:17:4".into()),
            &mut captured_stderr,
        );
        let record = read_jsonl(&sink.log_path())?.remove(0);
        let encoded = serde_json::to_string(&record)?;
        assert!(!encoded.contains("unlabeled private query"));
        assert!(!String::from_utf8(captured_stderr)?.contains("unlabeled private query"));
        assert_eq!(record["payload"], REDACTED);
        assert_eq!(record["payload_kind"], "string");
        assert_eq!(record["payload_bytes"], payload.len());
        assert!(record.get("payload_sha256").is_none());
        assert!(record.get("backtrace").is_none());
        Ok(())
    }

    #[test]
    fn rotating_log_is_bounded_and_private() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let sink = DiagnosticSink::new(directory.path().to_path_buf(), "rotation-test".into(), 256);
        for index in 0..20 {
            sink.write_record(json!({
                "event": "rotation_test",
                "level": "WARN",
                "index": index,
                "message": "x".repeat(80),
            }))?;
        }
        assert!(sink.log_path().exists());
        assert!(rotated_path(&sink.log_path(), 1).exists());
        assert!(!rotated_path(&sink.log_path(), RETAINED_LOGS + 1).exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(sink.log_path())?.permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(
                fs::metadata(sink.diagnostics_dir())?.permissions().mode() & 0o777,
                0o700
            );
        }
        Ok(())
    }

    #[test]
    fn fail_stop_markers_use_a_fixed_bounded_slot_set() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let sink = DiagnosticSink::new(
            directory.path().to_path_buf(),
            "fail-stop-test".into(),
            DEFAULT_LOG_BYTES,
        );
        let record = sink.decorate(json!({
            "event": "process_fail_stop",
            "level": "ERROR",
            "reason_code": "embedding_engine_stalled",
        }));
        let mut marker = PathBuf::new();
        for _ in 0..(FAIL_STOP_MARKER_SLOTS * 3) {
            marker = sink.write_fail_stop_marker(&record)?;
        }
        let parsed: Value = serde_json::from_slice(&fs::read(&marker)?)?;
        assert_eq!(parsed["reason_code"], "embedding_engine_stalled");
        assert_eq!(parsed["correlation_id"], "fail-stop-test");
        let markers = evidence_files(&sink.diagnostics_dir(), "fail-stop-", ".json")?;
        assert!(markers.len() <= FAIL_STOP_MARKER_SLOTS);
        assert!(
            evidence_namespace_count(&sink.diagnostics_dir(), "fail-stop-")?
                <= FAIL_STOP_MARKER_SLOTS * 3
        );
        let marker_bytes = markers.iter().try_fold(0_u64, |total, path| {
            Ok::<_, anyhow::Error>(total + fs::metadata(path)?.len())
        })?;
        assert!(marker_bytes <= (FAIL_STOP_MARKER_SLOTS * (MAX_RECORD_BYTES + 1)) as u64);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(marker)?.permissions().mode() & 0o777, 0o600);
        }
        Ok(())
    }

    #[test]
    fn lock_contention_uses_fixed_bounded_emergency_slots() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let sink = DiagnosticSink::new(
            directory.path().to_path_buf(),
            "emergency-test".into(),
            DEFAULT_LOG_BYTES,
        );
        ensure_private_directory(&sink.diagnostics_dir())?;
        let lock_path = sink.diagnostics_dir().join(LOCK_FILE);
        let lock = open_private_file(&lock_path, false, false)?;
        bounded_locks::acquire_with_deadline(
            &lock,
            FileLockKind::Exclusive,
            bounded_locks::LockDeadline::immediate(),
            None,
        )?;
        for index in 0..(EMERGENCY_LOG_SLOTS * 3) {
            sink.write_record(json!({
                "event": "lock_contention",
                "level": "WARN",
                "index": index,
            }))?;
        }
        bounded_locks::release(&lock)?;

        let files = evidence_files(&sink.diagnostics_dir(), "emergency-", ".jsonl")?;
        assert!(files.len() <= EMERGENCY_LOG_SLOTS);
        assert!(
            evidence_namespace_count(&sink.diagnostics_dir(), "emergency-")?
                <= EMERGENCY_LOG_SLOTS * 3
        );
        let bytes = files.iter().try_fold(0_u64, |total, path| {
            Ok::<_, anyhow::Error>(total + fs::metadata(path)?.len())
        })?;
        assert!(bytes <= (EMERGENCY_LOG_SLOTS * (MAX_RECORD_BYTES + 1)) as u64);
        Ok(())
    }

    #[test]
    fn fail_stop_marker_attempt_has_a_fixed_wait_bound() {
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        let started = std::time::Instant::now();
        let completed = run_bounded_attempt(Duration::from_millis(20), move || {
            let _ = release_rx.recv();
        });
        let elapsed = started.elapsed();
        assert!(!completed);
        assert!(elapsed < Duration::from_secs(1));
        let _ = release_tx.send(());
    }

    #[test]
    fn fail_stop_reason_codes_are_an_exact_safe_contract() {
        assert_eq!(
            safe_reason_code("embedding_engine_stalled"),
            "embedding_engine_stalled"
        );
        assert_eq!(
            safe_reason_code("embedding_qualification_crash"),
            "embedding_qualification_crash"
        );
        assert_eq!(
            safe_reason_code(codestory_runtime::ACTIVATION_QUIESCENCE_FAIL_STOP),
            "activation_quiescence_timeout"
        );
        assert_eq!(
            safe_reason_code("unlabeled_private_query"),
            "unknown_fail_stop"
        );
    }

    #[test]
    fn bounded_slot_collision_fails_without_touching_shared_paths() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let lock_path = directory.path().join(".emergency-00.lock");
        let lock = open_private_file(&lock_path, false, false)?;
        bounded_locks::acquire_with_deadline(
            &lock,
            FileLockKind::Exclusive,
            bounded_locks::LockDeadline::immediate(),
            None,
        )?;

        let error = write_bounded_slot(
            directory.path(),
            "emergency",
            "jsonl",
            0,
            b"private bytes must not be written\n",
        )
        .expect_err("a busy bounded slot must fail closed");
        assert!(error.to_string().contains("slot is busy"));
        assert!(!directory.path().join("emergency-00.jsonl").exists());
        assert!(!directory.path().join(".emergency-00.tmp").exists());
        bounded_locks::release(&lock)?;
        Ok(())
    }

    fn evidence_files(directory: &Path, prefix: &str, suffix: &str) -> Result<Vec<PathBuf>> {
        fs::read_dir(directory)?
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(prefix) && name.ends_with(suffix))
            })
            .map(Ok)
            .collect()
    }

    fn evidence_namespace_count(directory: &Path, stem: &str) -> Result<usize> {
        Ok(fs::read_dir(directory)?
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.contains(stem))
            })
            .count())
    }

    fn read_jsonl(path: &Path) -> Result<Vec<Value>> {
        let mut contents = String::new();
        File::open(path)?.read_to_string(&mut contents)?;
        contents
            .lines()
            .map(|line| serde_json::from_str(line).context("parse diagnostic row"))
            .collect()
    }
}
