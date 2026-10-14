//! Opt-in, request-scoped developer replay. Enrollment gates diagnostics only.
use codestory_contracts::api::AgentPacketRequestDto;
use codestory_contracts::compilation::PacketCompilationPublicationV1;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const FILE_CAP: usize = 1024 * 1024;
const SCOPE_CAP: u64 = 16 * 1024;
// Eight bounded extents (including explicit refusal) x 51 proposals + winner replay.
// Up to seven authenticated page extents, one refusal fallback and final
// revalidation. Keep capture completeness aligned with the bounded compiler.
const MAX_MEASUREMENTS: usize =
    8 * codestory_agent::evidence_compiler::REPOSITORY_EVIDENCE_MAX_MEASURED_CANDIDATES_V1 + 1;
static TEMP: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplayScope {
    public_source: bool,
    project_root: PathBuf,
    question_sha256: String,
    publication: PacketCompilationPublicationV1,
    core_run_id: String,
    source_sha256: Vec<String>,
}

#[derive(Clone)]
pub struct PacketReplayEnrollment {
    destination: PathBuf,
    scope: ReplayScope,
}

fn replay_serialized_bytes(value: &impl Serialize) -> Option<usize> {
    struct Counter(usize);
    impl Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 += bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter(0);
    serde_json::to_writer(&mut counter, value).ok()?;
    Some(counter.0)
}

pub(crate) fn replay_digest(value: &impl Serialize) -> Option<String> {
    // Hash through a writer, without allocating a second serialized frame.
    struct HashWriter(Sha256);
    impl Write for HashWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.update(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut writer = HashWriter(Sha256::new());
    serde_json::to_writer(&mut writer, value).ok()?;
    Some(format!("{:x}", writer.0.finalize()))
}

fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

fn private_directory(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::symlink_metadata(path)
            .is_ok_and(|meta| meta.is_dir() && meta.permissions().mode() & 0o077 == 0)
    }
    // This optional plaintext diagnostic requires an independently proven
    // private directory; Unix mode bits cannot prove Windows ACL isolation.
    #[cfg(not(unix))]
    {
        let _ = path;
        false
    }
}

impl PacketReplayEnrollment {
    fn load(destination: PathBuf, question: &str) -> Option<Self> {
        if !destination.is_absolute() || !private_directory(destination.parent()?) {
            return None;
        }
        let path = sibling(&destination, ".replay-scope.json");
        let meta = std::fs::symlink_metadata(&path).ok()?;
        if !meta.is_file() || meta.len() > SCOPE_CAP {
            return None;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if meta.permissions().mode() & 0o077 != 0 {
                return None;
            }
        }
        let mut bytes = Vec::new();
        std::fs::File::open(path)
            .ok()?
            .take(SCOPE_CAP + 1)
            .read_to_end(&mut bytes)
            .ok()?;
        if bytes.len() as u64 > SCOPE_CAP {
            return None;
        }
        let scope: ReplayScope = serde_json::from_slice(&bytes).ok()?;
        let question_sha256 = format!("{:x}", Sha256::digest(question.trim().as_bytes()));
        if !scope.public_source
            || scope.question_sha256 != question_sha256
            || scope.source_sha256.is_empty()
            || scope.source_sha256.len() > 256
            || scope
                .source_sha256
                .iter()
                .any(|hash| hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()))
        {
            return None;
        }
        Some(Self { destination, scope })
    }

    pub(crate) fn for_request(
        project_root: &Path,
        request: &AgentPacketRequestDto,
    ) -> Option<Self> {
        let destination = super::trace_export::packet_step_trace_destination_from_env()?;
        let enrollment = Self::load(destination, &request.question)?;
        if std::fs::canonicalize(project_root).ok()?
            != std::fs::canonicalize(&enrollment.scope.project_root).ok()?
        {
            return None;
        }
        Some(enrollment)
    }

    pub(crate) fn allows(
        &self,
        prepared: &super::packet_compiler::PreparedPacketCompilationV1,
    ) -> bool {
        let snapshot = prepared.replay_snapshot();
        let Some(publication) = snapshot.get("publication") else {
            return false;
        };
        if serde_json::to_value(&self.scope.publication).ok().as_ref() != Some(publication) {
            return false;
        }
        // Every captured source extent must retain a hash from this explicit
        // public-source enrollment, including shortened alternatives.
        fn sources_allowed(value: &Value, hashes: &[String]) -> bool {
            match value {
                Value::Object(map) => {
                    if let Some(hash) = map.get("source_sha256")
                        && hash
                            .as_str()
                            .is_none_or(|h| hashes.iter().all(|allowed| allowed != h))
                    {
                        return false;
                    }
                    if map.contains_key("source")
                        && map.contains_key("stable_identity")
                        && map
                            .get("delivery")
                            .is_none_or(|d| d.get("source_sha256").is_none())
                    {
                        return false;
                    }
                    map.values().all(|v| sources_allowed(v, hashes))
                }
                Value::Array(values) => values.iter().all(|v| sources_allowed(v, hashes)),
                _ => true,
            }
        }
        sources_allowed(&snapshot, &self.scope.source_sha256)
    }

    fn publish(&self, suffix: &str, value: &Value) -> bool {
        let path = sibling(&self.destination, suffix);
        let bytes = match bounded_json(value, FILE_CAP) {
            Some(bytes) => bytes,
            None => serde_json::to_vec(&json!({"schema":"packet_replay_v1", "complete":false, "reason":"capture_byte_cap_exceeded"})).unwrap_or_default(),
        };
        publish_private_new(&path, &bytes).is_ok()
    }
}

pub(crate) struct PacketReplayTrace {
    enrollment: PacketReplayEnrollment,
    input: Value,
    extents: BTreeMap<String, Value>,
    measurements: Vec<Value>,
    complete: bool,
}

impl PacketReplayTrace {
    pub(crate) fn begin(
        enrollment: PacketReplayEnrollment,
        request: &AgentPacketRequestDto,
        prepared: &super::packet_compiler::PreparedPacketCompilationV1,
        base: &codestory_contracts::api::AgentPacketDto,
        descriptors: Value,
    ) -> Option<Self> {
        if !enrollment.allows(prepared)
            || base
                .answer
                .retrieval_trace
                .retrieval_publication
                .as_ref()
                .is_none_or(|publication| publication.core_run_id != enrollment.scope.core_run_id)
        {
            return None;
        }
        let input = json!({"scope":enrollment.scope, "request":request,
            "prepared":prepared.replay_snapshot(), "base_packet":base, "descriptor_queries":descriptors});
        Some(Self {
            enrollment,
            input,
            extents: BTreeMap::new(),
            measurements: Vec::new(),
            complete: true,
        })
    }

    pub(crate) fn record(
        &mut self,
        prepared: &super::packet_compiler::PreparedPacketCompilationV1,
        before: &codestory_contracts::api::AgentPacketDto,
        after: &codestory_contracts::api::AgentPacketDto,
        outcome: &Result<Option<usize>, codestory_contracts::api::ApiError>,
    ) {
        if self.measurements.len() >= MAX_MEASUREMENTS {
            self.complete = false;
            return;
        }
        let extent = prepared.replay_input();
        let hash = replay_digest(extent).unwrap_or_default();
        self.extents
            .entry(hash.clone())
            .or_insert_with(|| json!(extent));
        self.measurements.push(json!({"ordinal":self.measurements.len(), "extent_sha256":hash,
            "support_rows":before.support.len(), "support_sha256":replay_digest(&before.support),
            "before_internal_budget_sha256":replay_digest(before), "after_internal_budget_sha256":replay_digest(after),
            "before_internal_budget_bytes":replay_serialized_bytes(before), "after_internal_budget_bytes":replay_serialized_bytes(after),
            "internal_support_unchanged":before.support == after.support,
            "outcome":match outcome { Ok(Some(bytes)) => json!({"kind":"measured", "bytes":bytes}),
                Ok(None) => json!({"kind":"refused"}), Err(error) => json!({"kind":"error", "message":format!("{error:?}")}) }}));
    }

    pub(crate) fn finish(
        self,
        selected: Option<&codestory_contracts::api::AgentPacketDto>,
        error: Option<String>,
    ) {
        let selected_digest = selected.and_then(replay_digest);
        let selected_measurement_ordinal = selected_digest.as_ref().and_then(|digest| {
            self.measurements.iter().rposition(|row| {
                row["after_internal_budget_sha256"].as_str() == Some(digest.as_str())
            })
        });
        let value = json!({"schema":"packet_replay_v1", "complete":self.complete && error.is_none(),
            "input":self.input, "extents":self.extents, "measurements":self.measurements,
            "selected_packet":selected, "selected_measurement_ordinal":selected_measurement_ordinal,
            "allocation_family_and_row_limit":"derive_by_replaying_ordered_pure_compiler_proposals",
            "error":error});
        let _ = self.enrollment.publish(".runtime-replay.json", &value);
    }
}

/// Transport-owned exact cost plan. A matching completed runtime witness must
/// already exist; cached or failed runtime captures never qualify as replay.
pub fn write_packet_stdio_replay_from_env(
    question: &str,
    core_generation: &str,
    core_run: &str,
    retrieval_generation: &str,
    current_internal_packet_id: &str,
    payload: impl FnOnce() -> Option<Value>,
) {
    let Some(destination) = super::trace_export::packet_step_trace_destination_from_env() else {
        return;
    };
    let Some(enrollment) = PacketReplayEnrollment::load(destination, question) else {
        return;
    };
    write_stdio_replay(
        enrollment,
        core_generation,
        core_run,
        retrieval_generation,
        current_internal_packet_id,
        payload,
    );
}

fn write_stdio_replay(
    enrollment: PacketReplayEnrollment,
    core_generation: &str,
    core_run: &str,
    retrieval_generation: &str,
    current_internal_packet_id: &str,
    payload: impl FnOnce() -> Option<Value>,
) {
    if enrollment.scope.publication.core_generation_id != core_generation
        || enrollment.scope.core_run_id != core_run
        || enrollment.scope.publication.retrieval_generation.as_deref()
            != Some(retrieval_generation)
    {
        return;
    }
    let path = sibling(&enrollment.destination, ".runtime-replay.json");
    let Ok(file) = std::fs::File::open(path) else {
        return;
    };
    let mut bytes = Vec::new();
    if file
        .take((FILE_CAP + 1) as u64)
        .read_to_end(&mut bytes)
        .is_err()
        || bytes.len() > FILE_CAP
    {
        return;
    }
    let Ok(runtime) = serde_json::from_slice::<Value>(&bytes) else {
        return;
    };
    if runtime.get("complete") != Some(&Value::Bool(true))
        || runtime
            .pointer("/selected_packet/packet_id")
            .and_then(Value::as_str)
            != Some(current_internal_packet_id)
        || runtime.pointer("/input/scope") != serde_json::to_value(&enrollment.scope).ok().as_ref()
    {
        return;
    }
    let Some(payload) = payload() else {
        return;
    };
    let _ = enrollment.publish(".stdio-replay.json", &json!({"schema":"packet_stdio_replay_v1",
        "complete":true,"runtime_sha256":format!("{:x}",Sha256::digest(&bytes)), "payload":payload}));
}

fn bounded_json(value: &Value, cap: usize) -> Option<Vec<u8>> {
    struct Bounded {
        bytes: Vec<u8>,
        cap: usize,
    }
    impl Write for Bounded {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > self.cap.saturating_sub(self.bytes.len()) {
                return Err(std::io::Error::other("capture byte cap"));
            }
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut writer = Bounded {
        bytes: Vec::new(),
        cap,
    };
    serde_json::to_writer(&mut writer, value).ok()?;
    Some(writer.bytes)
}

fn publish_private_new(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if !path.parent().is_some_and(private_directory) {
        return Err(std::io::Error::other("non-private capture directory"));
    }
    let parent = path.parent().unwrap();
    let temp = parent.join(format!(
        ".packet-replay-{}-{}.tmp",
        std::process::id(),
        TEMP.fetch_add(1, Ordering::Relaxed)
    ));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temp)?;
    let result = (|| {
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        // Atomic fresh publication: hard_link cannot replace an existing name.
        std::fs::hard_link(&temp, path)?;
        Ok(())
    })();
    let _ = std::fs::remove_file(temp);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn serializer_refuses_overflow_without_partial_frame() {
        let value = json!({"source":"é".repeat(80)});
        let bytes = serde_json::to_vec(&value).unwrap();
        assert_eq!(bounded_json(&value, bytes.len()).unwrap(), bytes);
        assert!(bounded_json(&value, bytes.len() - 1).is_none());
    }
    #[test]
    #[cfg(unix)]
    fn private_publication_never_overwrites_and_refuses_public_directory() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = dir.path().join("capture");
        publish_private_new(&path, b"one").unwrap();
        assert!(publish_private_new(&path, b"two").is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"one");
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(publish_private_new(&dir.path().join("second"), b"secret").is_err());
    }
    #[test]
    #[cfg(unix)]
    fn enrollment_rejects_private_question_and_unenrolled_source_extents() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let destination = dir.path().join("trace.json");
        let scope_path = sibling(&destination, ".replay-scope.json");
        let publication = PacketCompilationPublicationV1 {
            project_id: "project".into(),
            core_generation_id: "core".into(),
            retrieval_generation: Some("retrieval".into()),
        };
        let mut scope = ReplayScope {
            public_source: true,
            project_root: dir.path().into(),
            question_sha256: format!("{:x}", Sha256::digest(b"public question")),
            publication: publication.clone(),
            core_run_id: "run".into(),
            source_sha256: vec!["a".repeat(64)],
        };
        let save = |scope: &ReplayScope| {
            std::fs::write(&scope_path, serde_json::to_vec(scope).unwrap()).unwrap();
            std::fs::set_permissions(&scope_path, std::fs::Permissions::from_mode(0o600)).unwrap();
        };
        save(&scope);
        assert!(PacketReplayEnrollment::load(destination.clone(), "private question").is_none());
        let enrollment =
            PacketReplayEnrollment::load(destination.clone(), "public question").unwrap();
        let input = codestory_contracts::compilation::PacketCompilationInputV1 {
            contract_version: 1,
            publication,
            admissions: vec![],
            sources: vec![],
            relations: vec![],
            ambiguities: vec![],
            admission_gaps: vec![],
        };
        let prepared =
            super::super::packet_compiler::PreparedPacketCompilationV1::from_fixture_input(input);
        let mut snapshot = prepared.replay_snapshot();
        snapshot["source_refusals"] = json!(["fixture refusal"]);
        snapshot["file_navigation_paths"] = json!({"node":"public/file.rs"});
        snapshot["requested_source_identities"] = json!(["node"]);
        let restored: super::super::packet_compiler::PreparedPacketCompilationV1 =
            serde_json::from_value(snapshot.clone()).unwrap();
        assert_eq!(restored.replay_snapshot(), snapshot);
        assert!(enrollment.allows(&restored));
        let source = json!({"stable_identity":"node","path":"public/file.rs","start_line":1,"end_line":1,"source":"public body","parser_completeness":"complete","delivery":{"requested_range":{"start_line":1,"end_line":1},"source_sha256":"b".repeat(64),"remaining_source_bytes":0,"next_start_line":null,"blocked_line_rendered_bytes":null}});
        snapshot["input"]["sources"] = json!([source.clone()]);
        let refused: super::super::packet_compiler::PreparedPacketCompilationV1 =
            serde_json::from_value(snapshot.clone()).unwrap();
        assert!(!enrollment.allows(&refused));
        snapshot["input"]["sources"][0]["delivery"]["source_sha256"] = json!("a".repeat(64));
        snapshot["source_page_alternatives"] = json!({"512":[source]});
        let alternative_refused: super::super::packet_compiler::PreparedPacketCompilationV1 =
            serde_json::from_value(snapshot).unwrap();
        assert!(!enrollment.allows(&alternative_refused));
        scope.public_source = false;
        save(&scope);
        assert!(PacketReplayEnrollment::load(destination.clone(), "public question").is_none());
        scope.public_source = true;
        save(&scope);
        std::fs::set_permissions(&scope_path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(PacketReplayEnrollment::load(destination, "public question").is_none());
    }

    #[test]
    #[cfg(unix)]
    fn stdio_capture_joins_current_packet_and_never_evaluates_mismatched_payload() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let enrollment = PacketReplayEnrollment {
            destination: dir.path().join("trace"),
            scope: ReplayScope {
                public_source: true,
                project_root: dir.path().into(),
                question_sha256: "q".into(),
                publication: PacketCompilationPublicationV1 {
                    project_id: "p".into(),
                    core_generation_id: "core".into(),
                    retrieval_generation: Some("retrieval".into()),
                },
                core_run_id: "run".into(),
                source_sha256: vec!["a".repeat(64)],
            },
        };
        let runtime = json!({"complete":true,"input":{"scope":enrollment.scope},"selected_packet":{"packet_id":"current-packet"}});
        assert!(enrollment.publish(".runtime-replay.json", &runtime));
        let evaluated = std::cell::Cell::new(false);
        write_stdio_replay(
            enrollment.clone(),
            "core",
            "run",
            "retrieval",
            "other-packet",
            || {
                evaluated.set(true);
                Some(json!({"private":"must not copy"}))
            },
        );
        assert!(!evaluated.get());
        let output = sibling(&enrollment.destination, ".stdio-replay.json");
        assert!(!output.exists());
        write_stdio_replay(
            enrollment.clone(),
            "core",
            "other-run",
            "retrieval",
            "current-packet",
            || {
                evaluated.set(true);
                Some(json!({}))
            },
        );
        assert!(!evaluated.get());
        write_stdio_replay(
            enrollment.clone(),
            "core",
            "run",
            "retrieval",
            "current-packet",
            || Some(json!({"measured_bytes":123,"publication_meta":{"operation":"actual"}})),
        );
        let retained = std::fs::read(&output).unwrap();
        let capsule: Value = serde_json::from_slice(&retained).unwrap();
        assert_eq!(capsule["payload"]["measured_bytes"], 123);
        let runtime_bytes =
            std::fs::read(sibling(&enrollment.destination, ".runtime-replay.json")).unwrap();
        assert_eq!(
            capsule["runtime_sha256"],
            format!("{:x}", Sha256::digest(runtime_bytes))
        );
        write_stdio_replay(
            enrollment,
            "core",
            "run",
            "retrieval",
            "current-packet",
            || Some(json!({"measured_bytes":456})),
        );
        assert_eq!(std::fs::read(&output).unwrap(), retained);
    }
}
