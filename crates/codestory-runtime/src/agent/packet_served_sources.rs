//! Product-issued, immutable metadata for the source rows in a finalized public
//! Complete packet. No source body is stored or loaded before admission.
use crate::AppController;
use codestory_contracts::{
    api::{AgentPacketRequestDto, ApiError},
    compilation::{
        INTERIM_MAX_ADMITTED_CANDIDATES, INTERIM_MAX_ADMITTED_SOURCE_BYTES,
        PacketHydratedSourceRangeV1, PacketSourceDeliveryV1,
    },
    packet_projection_v3::{EvidenceKindV3Dto, PacketProjectionV3Dto, PublicationIdentityV3Dto},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::Read,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
const RECORD_MAX: usize = 64 * 1024;
const SCOPE_MAX: u64 = 128 * 1024 * 1024;
const TOTAL_MAX: u64 = 512 * 1024 * 1024;
const LEASE_SECONDS: u64 = 3600;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ServedSourceWitness {
    pub(crate) node_id: String,
    pub(crate) path: String,
    pub(crate) start_line: u32,
    pub(crate) end_line: u32,
    pub(crate) delivery: PacketSourceDeliveryV1,
    pub(crate) rendered_bytes: usize,
    pub(crate) public_source_sha256: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ServedSourceRecord {
    version: u32,
    caller: String,
    request_id: String,
    question_sha256: String,
    publication: PublicationIdentityV3Dto,
    issued_at: u64,
    expires_at: u64,
    pub(crate) witnesses: Vec<ServedSourceWitness>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SignedRecord {
    record: ServedSourceRecord,
    mac: String,
}
fn refused() -> ApiError {
    ApiError::invalid_argument(
        "Issued packet source record is missing, invalid, expired, or foreign.",
    )
}
fn io_error(_: impl std::fmt::Display) -> ApiError {
    ApiError::internal("Packet source record storage refused.")
}
fn now() -> Result<u64, ApiError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .map_err(io_error)
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
// HMAC-SHA256, with a fixed 32-byte private random key. Metadata bytes never
// authenticate themselves; the caller supplies only an opaque parent identity.
fn mac(key: &[u8; 32], bytes: &[u8]) -> String {
    let mut inner = [0x36_u8; 64];
    let mut outer = [0x5c_u8; 64];
    for i in 0..32 {
        inner[i] ^= key[i];
        outer[i] ^= key[i];
    }
    let mut h = Sha256::new();
    h.update(inner);
    h.update(b"codestory.served-source-record.v1\0");
    h.update(bytes);
    let mut o = Sha256::new();
    o.update(outer);
    o.update(h.finalize());
    format!("{:x}", o.finalize())
}
fn equal_mac(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |v, (x, y)| v | (x ^ y)) == 0
}
fn root(controller: &AppController) -> PathBuf {
    controller
        .runtime_config
        .cache_root
        .join("packet-served-sources-v1")
}
fn scope_id(project: &str, caller: &str) -> String {
    digest(&serde_json::to_vec(&(project, caller)).expect("string tuple"))
}
fn record_path(root: &Path, project: &str, caller: &str, parent: &str) -> PathBuf {
    root.join(scope_id(project, caller))
        .join(format!("{}.json", digest(parent.as_bytes())))
}
fn private_directory(path: &Path) -> Result<(), ApiError> {
    let mut b = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        b.mode(0o700);
    }
    match b.create(path) {
        Ok(()) => (),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
        Err(e) => return Err(io_error(e)),
    }
    let m = fs::symlink_metadata(path).map_err(io_error)?;
    if !m.is_dir() || m.file_type().is_symlink() {
        return Err(refused());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if m.permissions().mode() & 0o077 != 0 {
            return Err(refused());
        }
    }
    Ok(())
}
fn bounded_file(path: &Path, max: usize) -> Result<Vec<u8>, ApiError> {
    let m = fs::symlink_metadata(path).map_err(io_error)?;
    if !m.is_file() || m.file_type().is_symlink() || m.len() > max as u64 {
        return Err(refused());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if m.permissions().mode() & 0o077 != 0 {
            return Err(refused());
        }
    }
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(io_error)?
        .take(max as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    if bytes.len() > max {
        return Err(refused());
    }
    Ok(bytes)
}
fn key(root: &Path, create: bool) -> Result<[u8; 32], ApiError> {
    let path = root.join("key");
    if create && !path.try_exists().map_err(io_error)? {
        let mut bytes = [0; 32];
        bytes[..16].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
        bytes[16..].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
        codestory_workspace::atomic_file::publish_new_private_file_atomic(
            &path,
            "packet-source-key",
            &bytes,
        )
        .map_err(io_error)?;
    }
    bounded_file(&path, 32)?.try_into().map_err(|_| refused())
}
fn storage_lock(root: &Path) -> Result<File, ApiError> {
    let path = root.join("writer.lock");
    if path.try_exists().map_err(io_error)? {
        bounded_file(&path, 0)?;
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path).map_err(io_error)?;
    file.try_lock().map_err(io_error)?;
    Ok(file)
}
fn inventory(root: &Path, current_scope: &Path) -> Result<(u64, u64), ApiError> {
    inventory_with_limit(root, current_scope, 8192)
}
fn inventory_with_limit(
    root: &Path,
    current_scope: &Path,
    entry_limit: usize,
) -> Result<(u64, u64), ApiError> {
    let mut entries = 0usize;
    let mut total = 0u64;
    let mut scope = 0u64;
    for entry in fs::read_dir(root).map_err(io_error)? {
        entries += 1;
        if entries > entry_limit {
            return Err(io_error("inventory entry bound"));
        }
        let entry = entry.map_err(io_error)?;
        let path = entry.path();
        let m = fs::symlink_metadata(&path).map_err(io_error)?;
        if m.file_type().is_symlink() {
            return Err(refused());
        }
        if m.is_file() {
            total = total.checked_add(m.len()).ok_or_else(refused)?;
            continue;
        }
        if !m.is_dir() {
            return Err(refused());
        }
        for child in fs::read_dir(&path).map_err(io_error)? {
            entries += 1;
            if entries > entry_limit {
                return Err(io_error("inventory entry bound"));
            }
            let child = child.map_err(io_error)?;
            let m = fs::symlink_metadata(child.path()).map_err(io_error)?;
            if !m.is_file() || m.file_type().is_symlink() {
                return Err(refused());
            }
            total = total.checked_add(m.len()).ok_or_else(refused)?;
            if path == current_scope {
                scope = scope.checked_add(m.len()).ok_or_else(refused)?;
            }
        }
    }
    Ok((scope, total))
}
fn source_measurement(text: &str, start: u32, end: u32) -> Result<usize, ApiError> {
    let body = text
        .strip_prefix("```text\n")
        .and_then(|t| t.strip_suffix("\n```"))
        .ok_or_else(refused)?;
    let mut bytes = "```text\n".len() + 3;
    let mut count = 0u32;
    for line in body.split('\n') {
        let (prefix, source) = line.split_once(" | ").ok_or_else(refused)?;
        let marker = prefix.get(..1).ok_or_else(refused)?;
        let n = prefix
            .get(1..)
            .ok_or_else(refused)?
            .trim()
            .parse::<u32>()
            .map_err(|_| refused())?;
        if n != start.checked_add(count).ok_or_else(refused)?
            || marker != if count == 0 { ">" } else { " " }
        {
            return Err(refused());
        }
        bytes += format!("{}{:>5} | ", marker, n).len() + source.len() + 1;
        count += 1;
    }
    if count == 0 || start.checked_add(count - 1) != Some(end) || bytes > 4096 {
        return Err(refused());
    }
    Ok(bytes)
}
fn validate_record(
    record: &ServedSourceRecord,
    publication: &PublicationIdentityV3Dto,
    caller: &str,
    parent: &str,
    question: &str,
    at: u64,
) -> Result<(), ApiError> {
    if record.version != 1
        || record.caller != caller
        || record.request_id != parent
        || record.publication != *publication
        || record.question_sha256 != digest(question.as_bytes())
        || record.issued_at > at
        || record.expires_at <= at
        || record.expires_at
            != record
                .issued_at
                .checked_add(LEASE_SECONDS)
                .ok_or_else(refused)?
        || record.witnesses.len() > INTERIM_MAX_ADMITTED_CANDIDATES
    {
        return Err(refused());
    }
    let mut identities = std::collections::BTreeSet::new();
    let mut bytes = 0usize;
    for w in &record.witnesses {
        if w.node_id.parse::<i64>().is_err()
            || !identities.insert(w.node_id.clone())
            || !w.delivery.matches_delivery(w.start_line, w.end_line)
            || w.rendered_bytes == 0
            || w.rendered_bytes > 4096
            || w.public_source_sha256.len() != 64
            || w.path.is_empty()
        {
            return Err(refused());
        }
        bytes = bytes.checked_add(w.rendered_bytes).ok_or_else(refused)?;
    }
    if bytes > INTERIM_MAX_ADMITTED_SOURCE_BYTES {
        return Err(refused());
    }
    Ok(())
}
fn current_publication(controller: &AppController) -> Result<PublicationIdentityV3Dto, ApiError> {
    let core = controller.active_core_publication().ok_or_else(refused)?;
    let r = super::retrieval_primary::active_pinned_retrieval_publication(controller)
        .ok_or_else(refused)?;
    use codestory_contracts::packet_projection_v3::{
        CorePublicationIdentityV3Dto, IdentityTextV3, RetrievalPublicationIdentityV3Dto,
        Sha256DigestV3Dto,
    };
    let id = |s: String| IdentityTextV3::new(s).map_err(io_error);
    Ok(PublicationIdentityV3Dto {
        core: CorePublicationIdentityV3Dto {
            project_id: id(codestory_workspace::project_identity_v3(
                &controller.require_project_root()?,
            )
            .project_id)?,
            generation_id: id(core.generation_id)?,
            run_id: id(core.run_id)?,
        },
        retrieval: Some(RetrievalPublicationIdentityV3Dto {
            core_generation_id: id(r.core_generation_id)?,
            core_run_id: id(r.core_run_id)?,
            retrieval_generation: id(r.retrieval_generation)?,
            retrieval_input_sha256: Sha256DigestV3Dto::new(r.retrieval_input_hash)
                .map_err(io_error)?,
            semantic_generation: id(r.semantic_generation)?,
        }),
    })
}
/// Missing legacy records retain the old refusal path. An existing invalid or
/// expired record is never silently replaced or treated as authenticated history.
pub(crate) fn load(
    controller: &AppController,
    caller: &str,
    request: &AgentPacketRequestDto,
) -> Result<Option<ServedSourceRecord>, ApiError> {
    let Some(parent) = request.parent_packet_id.as_deref() else {
        return Ok(None);
    };
    let publication = current_publication(controller)?;
    let root = root(controller);
    load_record_or_legacy(
        &root,
        &publication,
        caller,
        parent,
        &request.question,
        now()?,
    )
}
fn load_record_or_legacy(
    root: &Path,
    publication: &PublicationIdentityV3Dto,
    caller: &str,
    parent: &str,
    question: &str,
    at: u64,
) -> Result<Option<ServedSourceRecord>, ApiError> {
    let path = record_path(root, publication.core.project_id.as_str(), caller, parent);
    if !path.try_exists().map_err(io_error)? {
        // A discriminator only distinguishes issued history from legacy IDs;
        // it does not authenticate anything. The signed record still must exist.
        return if parent.starts_with("psr1:") {
            Err(refused())
        } else {
            Ok(None)
        };
    }
    private_directory(root)?;
    private_directory(path.parent().ok_or_else(refused)?)?;
    read_record(root, publication, caller, parent, question, at).map(Some)
}
fn read_record(
    root: &Path,
    publication: &PublicationIdentityV3Dto,
    caller: &str,
    parent: &str,
    question: &str,
    at: u64,
) -> Result<ServedSourceRecord, ApiError> {
    let path = record_path(root, publication.core.project_id.as_str(), caller, parent);
    let signed: SignedRecord =
        serde_json::from_slice(&bounded_file(&path, RECORD_MAX)?).map_err(|_| refused())?;
    let expected = mac(
        &key(root, false)?,
        &serde_json::to_vec(&signed.record).map_err(io_error)?,
    );
    if !equal_mac(&expected, &signed.mac) {
        return Err(refused());
    }
    validate_record(&signed.record, publication, caller, parent, question, at)?;
    Ok(signed.record)
}
pub(crate) fn projection_preserves(
    record: &ServedSourceRecord,
    projection: &PacketProjectionV3Dto,
) -> bool {
    record
        .witnesses
        .iter()
        .all(|old| projection_preserves_witness(old, projection))
}
fn projection_preserves_witness(
    old: &ServedSourceWitness,
    projection: &PacketProjectionV3Dto,
) -> bool {
    let PacketProjectionV3Dto::Complete { evidence, .. } = projection else {
        return false;
    };
    evidence.as_slice().iter().any(|row| {
        if row.kind != EvidenceKindV3Dto::ExactSource
            || row.symbol_id.as_ref().map(|id| id.as_str()) != Some(old.node_id.as_str())
            || row.path.as_ref().map(|p| p.as_str()) != Some(old.path.as_str())
            || row.start_line != Some(old.start_line)
            || row.end_line.is_none_or(|end| end < old.end_line)
        {
            return false;
        }
        let Some(delivery) = row.source_delivery.as_ref() else {
            return false;
        };
        if delivery.source_sha256 != old.delivery.source_sha256
            || delivery.requested_range != old.delivery.requested_range
            || !delivery.matches_delivery(old.start_line, row.end_line.unwrap())
        {
            return false;
        }
        let Some(body) = row
            .summary
            .as_ref()
            .and_then(|s| s.as_str().strip_prefix("```text\n"))
            .and_then(|s| s.strip_suffix("\n```"))
        else {
            return false;
        };
        let prefix = body
            .split('\n')
            .take((old.end_line - old.start_line + 1) as usize)
            .collect::<Vec<_>>()
            .join("\n");
        digest(format!("```text\n{prefix}\n```").as_bytes()) == old.public_source_sha256
    })
}
impl ServedSourceWitness {
    pub(crate) fn matches(&self, source: &PacketHydratedSourceRangeV1) -> bool {
        let compact = crate::evidence_projection_v3::compact_numbered_source_page(
            &source.source,
            source.start_line,
            source.end_line,
        );
        source.stable_identity == format!("node:{}", self.node_id)
            && source.path == self.path
            && source.start_line == self.start_line
            && source.end_line == self.end_line
            && source.delivery.as_ref() == Some(&self.delivery)
            && source.source.len() == self.rendered_bytes
            && compact
                .as_ref()
                .is_some_and(|s| digest(s.as_bytes()) == self.public_source_sha256)
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PacketSourceIssuanceSeal {
    caller: String,
    identity: codestory_contracts::packet_projection_v3::PacketRequestIdentityV3Dto,
    publication: PublicationIdentityV3Dto,
    witnesses: Vec<ServedSourceWitness>,
}
fn witnesses(projection: &PacketProjectionV3Dto) -> Result<Vec<ServedSourceWitness>, ApiError> {
    let PacketProjectionV3Dto::Complete { evidence, .. } = projection else {
        return Ok(Vec::new());
    };
    let mut result = Vec::new();
    for row in evidence
        .as_slice()
        .iter()
        .filter(|r| r.kind == EvidenceKindV3Dto::ExactSource)
    {
        let delivery = row.source_delivery.clone().ok_or_else(refused)?;
        let text = row.summary.as_ref().ok_or_else(refused)?.as_str();
        let start = row.start_line.ok_or_else(refused)?;
        let end = row.end_line.ok_or_else(refused)?;
        result.push(ServedSourceWitness {
            node_id: row
                .symbol_id
                .as_ref()
                .ok_or_else(refused)?
                .as_str()
                .to_owned(),
            path: row.path.as_ref().ok_or_else(refused)?.as_str().to_owned(),
            start_line: start,
            end_line: end,
            delivery,
            rendered_bytes: source_measurement(text, start, end)?,
            public_source_sha256: digest(text.as_bytes()),
        });
    }
    Ok(result)
}
// Private provenance travels beside the product, never through serialized DTOs.
// No write occurs while projecting or measuring rejected previews.
pub(crate) fn packet_request_id(caller: Option<&str>, recordable: bool, legacy: &str) -> String {
    if caller == Some("codestory-cli") && recordable {
        format!("psr1:{}", uuid::Uuid::new_v4())
    } else {
        legacy.to_owned()
    }
}
pub(crate) fn seal(
    caller: &str,
    projection: &PacketProjectionV3Dto,
) -> Option<PacketSourceIssuanceSeal> {
    let PacketProjectionV3Dto::Complete {
        identity,
        publication,
        ..
    } = projection
    else {
        return None;
    };
    Some(PacketSourceIssuanceSeal {
        caller: caller.to_owned(),
        identity: identity.clone(),
        publication: publication.clone(),
        witnesses: witnesses(projection).ok()?,
    })
}
pub(crate) fn commit(
    controller: &AppController,
    caller: &str,
    product: &crate::PacketEvidenceProductV3,
) -> Result<(), ApiError> {
    let PacketProjectionV3Dto::Complete {
        identity,
        publication,
        ..
    } = &product.projection
    else {
        return Ok(());
    };
    // Final protected-source validation precedes every issuance/legacy path.
    if product
        .protected_source_witnesses
        .iter()
        .any(|old| !projection_preserves_witness(old, &product.projection))
    {
        return Err(refused());
    }
    if !identity.request_id.as_str().starts_with("psr1:") {
        return Ok(());
    }
    let seal = product.source_issuance_seal.as_ref().ok_or_else(refused)?;
    let final_witnesses = witnesses(&product.projection)?;
    if seal.caller != caller
        || seal.identity != *identity
        || seal.publication != *publication
        || final_witnesses.iter().any(|w| !seal.witnesses.contains(w))
    {
        return Err(refused());
    }
    controller.with_complete_core_snapshot(|core| {
        super::retrieval_primary::with_pinned_retrieval_publication_value(
            controller,
            &core.generation_id,
            &core.run_id,
            || {
                if current_publication(controller)? != *publication {
                    return Err(refused());
                }
                let at = now()?;
                let record = ServedSourceRecord {
                    version: 1,
                    caller: caller.to_owned(),
                    request_id: identity.request_id.as_str().to_owned(),
                    question_sha256: identity.question_sha256.as_str().to_owned(),
                    publication: publication.clone(),
                    issued_at: at,
                    expires_at: at.checked_add(LEASE_SECONDS).ok_or_else(refused)?,
                    witnesses: final_witnesses,
                };
                validate_record_without_question(&record)?;
                persist(&root(controller), record)
            },
        )
        .map(|_| ())
    })
}
fn validate_record_without_question(record: &ServedSourceRecord) -> Result<(), ApiError> {
    // The projection builder already bound the question digest; the loader
    // compares it to the actual continuation question. Validate remaining shape.
    if record.question_sha256.len() != 64
        || record.request_id.is_empty()
        || record.caller.is_empty()
        || record.witnesses.len() > 16
    {
        return Err(refused());
    }
    let mut ids = std::collections::BTreeSet::new();
    let mut total = 0;
    for w in &record.witnesses {
        if !ids.insert(w.node_id.clone())
            || w.node_id.parse::<i64>().is_err()
            || !w.delivery.matches_delivery(w.start_line, w.end_line)
            || w.rendered_bytes == 0
            || w.rendered_bytes > 4096
        {
            return Err(refused());
        }
        total += w.rendered_bytes;
    }
    if total > INTERIM_MAX_ADMITTED_SOURCE_BYTES {
        return Err(refused());
    }
    Ok(())
}
fn persist(root: &Path, record: ServedSourceRecord) -> Result<(), ApiError> {
    private_directory(root)?;
    let _lock = storage_lock(root)?;
    let scope = root.join(scope_id(
        record.publication.core.project_id.as_str(),
        &record.caller,
    ));
    private_directory(&scope)?;
    let path = record_path(
        root,
        record.publication.core.project_id.as_str(),
        &record.caller,
        &record.request_id,
    );
    let k = key(root, true)?;
    if path.try_exists().map_err(io_error)? {
        let previous: SignedRecord =
            serde_json::from_slice(&bounded_file(&path, RECORD_MAX)?).map_err(|_| refused())?;
        if !equal_mac(
            &previous.mac,
            &mac(&k, &serde_json::to_vec(&previous.record).map_err(io_error)?),
        ) || previous.record.expires_at <= now()?
        {
            return Err(refused());
        }
        let mut expected = record.clone();
        expected.issued_at = previous.record.issued_at;
        expected.expires_at = previous.record.expires_at;
        if serde_json::to_vec(&expected).map_err(io_error)?
            != serde_json::to_vec(&previous.record).map_err(io_error)?
        {
            return Err(refused());
        }
        return Ok(());
    }
    let signed = SignedRecord {
        mac: mac(&k, &serde_json::to_vec(&record).map_err(io_error)?),
        record,
    };
    let bytes = serde_json::to_vec(&signed).map_err(io_error)?;
    let (scoped, total) = inventory(root, &scope)?;
    if bytes.len() > RECORD_MAX
        || scoped + bytes.len() as u64 > SCOPE_MAX
        || total + bytes.len() as u64 > TOTAL_MAX
    {
        return Err(io_error("record cap"));
    }
    codestory_workspace::atomic_file::publish_new_private_file_atomic(
        &path,
        "packet-source-record",
        &bytes,
    )
    .map_err(io_error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use codestory_contracts::{
        compilation::{PacketParserCompletenessV1, PacketSourceRangeV1},
        packet_projection_v3::{
            CorePublicationIdentityV3Dto, IdentityTextV3, RetrievalPublicationIdentityV3Dto,
            Sha256DigestV3Dto,
        },
    };
    fn record() -> ServedSourceRecord {
        let id = |s: &str| IdentityTextV3::new(s.to_owned()).unwrap();
        let at = now().unwrap();
        ServedSourceRecord {
            version: 1,
            caller: "codestory-cli".into(),
            request_id: "psr1:actual-issued-operation".into(),
            question_sha256: digest(b"ordinary question"),
            issued_at: at,
            expires_at: at + LEASE_SECONDS,
            publication: PublicationIdentityV3Dto {
                core: CorePublicationIdentityV3Dto {
                    project_id: id("project"),
                    generation_id: id("core"),
                    run_id: id("run"),
                },
                retrieval: Some(RetrievalPublicationIdentityV3Dto {
                    core_generation_id: id("core"),
                    core_run_id: id("run"),
                    retrieval_generation: id("retrieval"),
                    retrieval_input_sha256: Sha256DigestV3Dto::new("a".repeat(64)).unwrap(),
                    semantic_generation: id("semantic"),
                }),
            },
            witnesses: vec![ServedSourceWitness {
                node_id: "17".into(),
                path: "source.rs".into(),
                start_line: 1,
                end_line: 1,
                delivery: PacketSourceDeliveryV1 {
                    requested_range: PacketSourceRangeV1 {
                        start_line: 1,
                        end_line: 3,
                    },
                    source_sha256: "b".repeat(64),
                    remaining_source_bytes: 12,
                    next_start_line: Some(2),
                    blocked_line_rendered_bytes: None,
                },
                rendered_bytes: 24,
                public_source_sha256: digest(b"```text\n>1 | row\n```"),
            }],
        }
    }
    #[test]
    fn served_source_record_immutable_cross_process_and_hostile_bindings() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("owned");
        let record = record();
        persist(&root, record.clone()).unwrap();
        let path = record_path(&root, "project", "codestory-cli", &record.request_id);
        let bytes = fs::read(&path).unwrap();
        persist(&root, record.clone()).unwrap();
        assert_eq!(
            fs::read(&path).unwrap(),
            bytes,
            "idempotent commit must not renew lease or overwrite"
        );
        let loaded = read_record(
            &root,
            &record.publication,
            &record.caller,
            &record.request_id,
            "ordinary question",
            record.issued_at,
        )
        .unwrap();
        assert_eq!(loaded.witnesses, record.witnesses);
        // A fresh process can consume the issued immutable record and private
        // key; no test hook inserts a historical record into production state.
        let expected_path = dir.path().join("expected.json");
        fs::write(&expected_path, serde_json::to_vec(&record).unwrap()).unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "agent::packet_served_sources::tests::served_source_record_cross_process_child",
                "--nocapture",
            ])
            .env("CODESTORY_UNIT_RECEIPT_ROOT", &root)
            .env("CODESTORY_UNIT_RECEIPT_EXPECTED", &expected_path)
            .status()
            .unwrap();
        assert!(status.success());
        assert!(
            read_record(
                &root,
                &record.publication,
                "foreign-transport",
                &record.request_id,
                "ordinary question",
                record.issued_at
            )
            .is_err()
        );
        assert!(
            read_record(
                &root,
                &record.publication,
                &record.caller,
                "unissued-parent",
                "ordinary question",
                record.issued_at
            )
            .is_err()
        );
        for mode in 0..9 {
            let mut mutated = record.clone();
            match mode {
                0 => mutated.caller = "foreign".into(),
                1 => mutated.request_id = "forged".into(),
                2 => mutated.question_sha256 = digest(b"different question"),
                3 => {
                    mutated.publication.core.project_id =
                        IdentityTextV3::new("foreign-root").unwrap()
                }
                4 => mutated.publication.core.run_id = IdentityTextV3::new("foreign-run").unwrap(),
                5 => {
                    mutated
                        .publication
                        .retrieval
                        .as_mut()
                        .unwrap()
                        .retrieval_generation = IdentityTextV3::new("stale-retrieval").unwrap()
                }
                6 => mutated.witnesses[0].end_line = 2,
                7 => mutated.witnesses[0].public_source_sha256 = digest(b"replaced last line"),
                _ => mutated.expires_at += 1,
            }
            let mut forged: SignedRecord = serde_json::from_slice(&bytes).unwrap();
            forged.record = mutated;
            fs::write(&path, serde_json::to_vec(&forged).unwrap()).unwrap();
            assert!(
                read_record(
                    &root,
                    &record.publication,
                    &record.caller,
                    &record.request_id,
                    "ordinary question",
                    record.issued_at
                )
                .is_err(),
                "mutation {mode}"
            );
        }
        fs::write(&path, &bytes).unwrap();
        assert!(
            read_record(
                &root,
                &record.publication,
                &record.caller,
                &record.request_id,
                "different question",
                record.issued_at
            )
            .is_err()
        );
        assert!(
            read_record(
                &root,
                &record.publication,
                &record.caller,
                &record.request_id,
                "ordinary question",
                record.expires_at
            )
            .is_err()
        );
        let mut changed = record.clone();
        changed.witnesses[0].path = "other.rs".into();
        assert!(persist(&root, changed).is_err());
        assert_eq!(fs::read(&path).unwrap(), bytes);
        // A correctly MACed but structurally invalid inventory also fails closed.
        let mut malformed = record.clone();
        malformed.witnesses.push(malformed.witnesses[0].clone());
        let signed = SignedRecord {
            mac: mac(
                &key(&root, false).unwrap(),
                &serde_json::to_vec(&malformed).unwrap(),
            ),
            record: malformed,
        };
        fs::write(&path, serde_json::to_vec(&signed).unwrap()).unwrap();
        assert!(
            read_record(
                &root,
                &record.publication,
                &record.caller,
                &record.request_id,
                "ordinary question",
                record.issued_at
            )
            .is_err()
        );
    }
    #[test]
    fn served_source_record_cross_process_child() {
        let Some(root) = std::env::var_os("CODESTORY_UNIT_RECEIPT_ROOT") else {
            return;
        };
        let record: ServedSourceRecord = serde_json::from_slice(
            &fs::read(std::env::var_os("CODESTORY_UNIT_RECEIPT_EXPECTED").unwrap()).unwrap(),
        )
        .unwrap();
        let read = read_record(
            Path::new(&root),
            &record.publication,
            &record.caller,
            &record.request_id,
            "ordinary question",
            record.issued_at,
        )
        .unwrap();
        assert_eq!(read.witnesses, record.witnesses);
    }
    #[test]
    fn served_source_record_missing_foreign_and_bounded_inventory_fail_closed() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("owned");
        let r = record();
        assert!(
            load_record_or_legacy(
                &root,
                &r.publication,
                &r.caller,
                &r.request_id,
                "ordinary question",
                r.issued_at
            )
            .is_err()
        );
        assert!(
            load_record_or_legacy(
                &root,
                &r.publication,
                &r.caller,
                "legacy-unissued",
                "ordinary question",
                r.issued_at
            )
            .unwrap()
            .is_none()
        );
        persist(&root, r.clone()).unwrap();
        assert!(
            load_record_or_legacy(
                &root,
                &r.publication,
                &r.caller,
                &r.request_id,
                "ordinary question",
                r.issued_at
            )
            .unwrap()
            .is_some()
        );
        assert!(
            load_record_or_legacy(
                &root,
                &r.publication,
                "foreign-transport",
                &r.request_id,
                "ordinary question",
                r.issued_at
            )
            .is_err()
        );
        let mut foreign = r.publication.clone();
        foreign.core.project_id = IdentityTextV3::new("foreign-project").unwrap();
        assert!(
            load_record_or_legacy(
                &root,
                &foreign,
                &r.caller,
                &r.request_id,
                "ordinary question",
                r.issued_at
            )
            .is_err()
        );
        assert!(
            inventory_with_limit(&root, &root.join(scope_id("project", "codestory-cli")), 1)
                .is_err()
        );
        let path = record_path(&root, "project", &r.caller, &r.request_id);
        fs::remove_file(path).unwrap();
        assert!(
            load_record_or_legacy(
                &root,
                &r.publication,
                &r.caller,
                &r.request_id,
                "ordinary question",
                r.issued_at
            )
            .is_err()
        );
    }

    #[test]
    fn served_source_record_exact_prefix_receipt_and_budget() {
        let aligned = "```text\n>    1 | éé\n     2 | \n```";
        let compact = "```text\n>1 | éé\n 2 | \n```";
        assert_eq!(source_measurement(compact, 1, 2).unwrap(), aligned.len());
        let mut witness = record().witnesses.remove(0);
        witness.end_line = 2;
        witness.rendered_bytes = aligned.len();
        witness.public_source_sha256 = digest(compact.as_bytes());
        witness.delivery.next_start_line = Some(3);
        let source = PacketHydratedSourceRangeV1 {
            callable_body: None,
            stable_identity: "node:17".into(),
            path: "source.rs".into(),
            symbol: None,
            start_line: 1,
            end_line: 2,
            source: aligned.into(),
            parser_completeness: PacketParserCompletenessV1::Complete,
            delivery: Some(witness.delivery.clone()),
        };
        assert!(witness.matches(&source));
        let mut changed = source.clone();
        changed.source = aligned.replace("éé", "éx");
        assert!(!witness.matches(&changed));
        changed = source.clone();
        changed.path = "foreign.rs".into();
        assert!(!witness.matches(&changed));
        changed = source.clone();
        changed.delivery.as_mut().unwrap().requested_range.end_line = 4;
        assert!(!witness.matches(&changed));
        let session = super::super::packet_candidate::PacketProofSession::new();
        let mut r = record();
        r.witnesses = vec![witness.clone()];
        session.protect_served_sources(r).unwrap();
        assert_eq!(
            session.receipts()[0].reserved_source_bytes as usize,
            aligned.len()
        );
        assert_eq!(*session.admitted_source_bytes.borrow(), aligned.len());
        assert_eq!(
            session.admit("node:optional", INTERIM_MAX_ADMITTED_SOURCE_BYTES),
            super::super::packet_candidate::PacketAdmissionDecision::SourceBudgetExceeded
        );
        assert_eq!(
            *session.admitted_source_bytes.borrow(),
            aligned.len(),
            "failed optional admission cannot refund protected reservation"
        );
        assert_eq!(
            source.delivery.as_ref().unwrap().next_start_line,
            Some(3),
            "unread tail is not protected as delivered"
        );
    }
    #[test]
    fn served_source_record_public_diversity_cannot_drop_protected_prefixes() {
        use crate::evidence_projection_v3::representation_budget_test_support as fixture;
        use codestory_contracts::{
            api::{AgentPacketRequestDto, SupportUnitKindDto},
            compilation::*,
        };
        let sources = (0..16)
            .map(|id| {
                let source =
                    format!("```text\n>    1 | fn ordinary_{id}() {{ operation(); }}\n```");
                PacketHydratedSourceRangeV1 {
                    callable_body: None,
                    stable_identity: format!("node:{id}"),
                    path: if id < 5 {
                        "shared.rs".into()
                    } else {
                        format!("source_{id}.rs")
                    },
                    symbol: Some(format!("ordinary_{id}")),
                    start_line: 1,
                    end_line: 1,
                    source,
                    parser_completeness: PacketParserCompletenessV1::Complete,
                    delivery: Some(PacketSourceDeliveryV1 {
                        requested_range: PacketSourceRangeV1 {
                            start_line: 1,
                            end_line: 1,
                        },
                        source_sha256: "b".repeat(64),
                        remaining_source_bytes: 0,
                        next_start_line: None,
                        blocked_line_rendered_bytes: None,
                    }),
                }
            })
            .collect::<Vec<_>>();
        let input = PacketCompilationInputV1 {
            contract_version: PACKET_COMPILATION_CONTRACT_VERSION_V1,
            publication: PacketCompilationPublicationV1 {
                project_id: "project-1".into(),
                core_generation_id: "core-generation-1".into(),
                retrieval_generation: Some("retrieval-generation-1".into()),
            },
            admissions: sources
                .iter()
                .enumerate()
                .map(|(id, source)| PacketAdmissionReceiptV1 {
                    packet_ordinal: id as u32,
                    stable_identity: source.stable_identity.clone(),
                    reserved_source_bytes: 512,
                    score_version: "ordinary".into(),
                    origin: PacketAdmissionOriginV1::Retrieval,
                })
                .collect(),
            sources,
            relations: (0..4)
                .map(|id| PacketDirectedRelationV1 {
                    relation_id: format!("edge-{id}"),
                    from_identity: format!("node:{id}"),
                    to_identity: format!("node:{}", id + 5),
                    relation_kind: PacketRelationKindV1::Call,
                    certainty: PacketRelationCertaintyV1::Certain,
                })
                .collect(),
            ambiguities: Vec::new(),
            admission_gaps: Vec::new(),
        };
        let request = AgentPacketRequestDto {
            question: "ordinary question".into(),
            probes: Vec::new(),
            parent_packet_id: None,
            option_ids: Vec::new(),
            core_generation_id: None,
            retrieval_generation: None,
            budget: codestory_contracts::api::PacketBudgetModeDto::Compact,
            latency_budget_ms: Some(18000),
        };
        let mut prior_input = input.clone();
        prior_input.sources.truncate(5);
        prior_input.admissions.truncate(5);
        prior_input.relations.clear();
        let (prior_packet, _) = fixture::default_packet(prior_input, &request);
        let prior = fixture::project_packet(&request, &prior_packet, |p| {
            Ok(serde_json::to_vec(p).unwrap().len())
        })
        .unwrap();
        let mut record = record();
        record.witnesses = witnesses(&prior.projection).unwrap();
        assert_eq!(record.witnesses.len(), 5);
        assert!(projection_preserves(&record, &prior.projection));
        // Deliberately overbound direct-projector control. The normal compiler
        // itself caps support; this models an adapter/finalization mutation,
        // rather than claiming normal compilation emits all of these rows.
        let sources = input.sources.clone();
        let (mut all_packet, _) = fixture::default_packet(input, &request);
        let relations = all_packet
            .support
            .iter()
            .filter(|r| r.kind == SupportUnitKindDto::TypedGraphEdge)
            .cloned()
            .collect::<Vec<_>>();
        all_packet.support = sources
            .iter()
            .map(|source| codestory_contracts::api::SupportUnitDto {
                source_delivery: source.delivery.clone(),
                id: format!("source:{}", source.stable_identity),
                kind: SupportUnitKindDto::SourceRange,
                summary: "ordinary source".into(),
                path: Some(source.path.clone()),
                symbol_id: source
                    .stable_identity
                    .strip_prefix("node:")
                    .map(str::to_owned),
                start_line: Some(source.start_line),
                end_line: Some(source.end_line),
                snippet: Some(source.source.clone()),
                edge_kind: None,
                from_symbol: None,
                to_symbol: None,
                query: None,
            })
            .collect();
        all_packet.support.rotate_left(5);
        all_packet.support.extend(relations);
        assert_eq!(
            all_packet
                .support
                .iter()
                .filter(|r| r.kind == SupportUnitKindDto::SourceRange)
                .count(),
            16
        );
        let projected = fixture::project_packet(&request, &all_packet, |p| {
            Ok(serde_json::to_vec(p).unwrap().len())
        })
        .unwrap();
        assert!(matches!(
            projected.projection,
            PacketProjectionV3Dto::Complete { .. }
        ));
        assert!(
            !projection_preserves(&record, &projected.projection),
            "public diversity selection must not authenticate omitted original rows"
        );
        let mut changed = prior.projection.clone();
        if let PacketProjectionV3Dto::Complete { evidence, .. } = &mut changed {
            let mut rows = evidence.as_slice().to_vec();
            rows[0].summary = Some(
                codestory_contracts::packet_projection_v3::SummaryTextV3::new(
                    "```text\n>1 | changed\n```",
                )
                .unwrap(),
            );
            *evidence = codestory_contracts::packet_projection_v3::BoundedVecV3::new(rows).unwrap();
        }
        assert!(
            !projection_preserves(&record, &changed),
            "same locator cannot replace the actual original bytes"
        );
        // Clearing an issuance seal cannot bypass protected-prefix validation,
        // even when the product returns to the legacy/no-new-record path.
        let dir = tempfile::tempdir().unwrap();
        let controller = AppController::new_with_owned_cache_root(dir.path());
        let mut unsealed = prior.clone();
        unsealed.protected_source_witnesses = record.witnesses.clone();
        unsealed.source_issuance_seal = None;
        unsealed.projection = changed;
        assert!(commit(&controller, "codestory-cli", &unsealed).is_err());
        assert!(
            fs::read_dir(dir.path()).unwrap().next().is_none(),
            "refused projection must not issue storage or keys"
        );
    }

    #[test]
    fn served_source_record_only_committing_recordable_transport_marks_ids() {
        assert!(packet_request_id(Some("codestory-cli"), true, "legacy").starts_with("psr1:"));
        assert_eq!(
            packet_request_id(Some("codestory-stdio"), true, "legacy"),
            "legacy"
        );
        assert_eq!(
            packet_request_id(Some("codestory-cli"), false, "legacy"),
            "legacy"
        );
        assert_eq!(packet_request_id(None, true, "legacy"), "legacy");
        // Mixed receipt-bearing source plus a legacy FILE source stays on its
        // original conservative path; no nonexistent issued parent is emitted.
        let input = codestory_contracts::compilation::PacketCompilationInputV1 {
            contract_version: 1,
            publication: codestory_contracts::compilation::PacketCompilationPublicationV1 {
                project_id: "p".into(),
                core_generation_id: "c".into(),
                retrieval_generation: None,
            },
            admissions: Vec::new(),
            sources: vec![
                PacketHydratedSourceRangeV1 {
                    callable_body: None,
                    stable_identity: "node:17".into(),
                    path: "f.rs".into(),
                    symbol: None,
                    start_line: 1,
                    end_line: 1,
                    source: "```text\n>    1 | row\n```".into(),
                    parser_completeness: PacketParserCompletenessV1::Complete,
                    delivery: Some(record().witnesses[0].delivery.clone()),
                },
                PacketHydratedSourceRangeV1 {
                    callable_body: None,
                    stable_identity: "path:legacy.rs".into(),
                    path: "legacy.rs".into(),
                    symbol: None,
                    start_line: 1,
                    end_line: 1,
                    source: "legacy".into(),
                    parser_completeness: PacketParserCompletenessV1::Complete,
                    delivery: None,
                },
            ],
            relations: Vec::new(),
            ambiguities: Vec::new(),
            admission_gaps: Vec::new(),
        };
        let mixed = super::super::packet_compiler::PreparedPacketCompilationV1::from_fixture_input(
            input.clone(),
        );
        assert!(!mixed.supports_served_source_record());
        let mut numeric = input;
        numeric.sources.pop();
        let numeric =
            super::super::packet_compiler::PreparedPacketCompilationV1::from_fixture_input(numeric);
        assert!(numeric.supports_served_source_record());
    }

    #[test]
    fn served_source_record_private_storage_refuses_aliases_and_oversize() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("owned");
        let r = record();
        persist(&root, r.clone()).unwrap();
        let path = record_path(&root, "project", "codestory-cli", &r.request_id);
        fs::write(&path, vec![0u8; RECORD_MAX + 1]).unwrap();
        assert!(bounded_file(&path, RECORD_MAX).is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::{PermissionsExt, symlink};
            fs::remove_file(&path).unwrap();
            symlink(root.join("key"), &path).unwrap();
            assert!(bounded_file(&path, RECORD_MAX).is_err());
            fs::remove_file(&path).unwrap();
            fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap();
            assert!(private_directory(&root).is_err());
        }
    }
}
