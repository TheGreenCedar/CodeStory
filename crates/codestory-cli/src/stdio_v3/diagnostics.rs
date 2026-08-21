use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;

pub(crate) const DIAGNOSTIC_ENTRY_MAX_BYTES_V3: usize = 1024 * 1024;
pub(crate) const DIAGNOSTIC_REGISTRY_MAX_BYTES_V3: usize = 8 * 1024 * 1024;
pub(crate) const DIAGNOSTIC_REGISTRY_MAX_ENTRIES_V3: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DiagnosticsBindingV3 {
    pub(crate) packet_id: String,
    pub(crate) project_id: String,
    pub(crate) publication_id: String,
    pub(crate) request_digest: String,
    pub(crate) wall_expiry_epoch_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DiagnosticsGrantV3 {
    pub(crate) uri: String,
    pub(crate) byte_length: usize,
    pub(crate) sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DiagnosticsReadErrorV3 {
    MalformedUri,
    CapabilityUnavailable,
    Internal,
}

impl DiagnosticsReadErrorV3 {
    pub(crate) const fn jsonrpc_code(self) -> i64 {
        match self {
            Self::MalformedUri => -32602,
            Self::CapabilityUnavailable => -32002,
            Self::Internal => -32603,
        }
    }
}

struct DiagnosticsEntryV3 {
    packet_id: String,
    token: [u8; 32],
    bytes: Arc<[u8]>,
    expires_at: Instant,
}

pub(crate) struct DiagnosticsRegistryV3 {
    secret: [u8; 32],
    entries: VecDeque<DiagnosticsEntryV3>,
    retained_bytes: usize,
}

impl DiagnosticsRegistryV3 {
    #[allow(dead_code)]
    pub(crate) fn new() -> Self {
        let first = Uuid::new_v4().into_bytes();
        let second = Uuid::new_v4().into_bytes();
        let mut seed = [0_u8; 32];
        seed[..16].copy_from_slice(&first);
        seed[16..].copy_from_slice(&second);
        let secret = Sha256::digest(seed).into();
        Self::new_with_secret(secret)
    }

    pub(crate) fn new_with_secret(secret: [u8; 32]) -> Self {
        Self {
            secret,
            entries: VecDeque::new(),
            retained_bytes: 0,
        }
    }

    pub(crate) fn register_at(
        &mut self,
        binding: DiagnosticsBindingV3,
        bytes: Vec<u8>,
        now: Instant,
    ) -> Result<DiagnosticsGrantV3, DiagnosticsReadErrorV3> {
        if bytes.len() > DIAGNOSTIC_ENTRY_MAX_BYTES_V3
            || !is_random_packet_id_v3(&binding.packet_id)
        {
            return Err(DiagnosticsReadErrorV3::Internal);
        }
        let token = capability_token_v3(&self.secret, &binding);
        let sha256 = sha256_hex_v3(&bytes);
        let byte_length = bytes.len();
        let uri = format!(
            "codestory://packet-diagnostics/{}/{}",
            binding.packet_id,
            hex_v3(&token)
        );
        let bytes: Arc<[u8]> = bytes.into();
        self.prune_expired_at(now);
        while self.entries.len() >= DIAGNOSTIC_REGISTRY_MAX_ENTRIES_V3
            || self.retained_bytes + bytes.len() > DIAGNOSTIC_REGISTRY_MAX_BYTES_V3
        {
            self.evict_oldest();
        }
        self.retained_bytes += bytes.len();
        self.entries.push_back(DiagnosticsEntryV3 {
            packet_id: binding.packet_id,
            token,
            bytes,
            expires_at: now + Duration::from_secs(10 * 60),
        });
        Ok(DiagnosticsGrantV3 {
            uri,
            byte_length,
            sha256,
        })
    }

    pub(crate) fn read_at(
        &mut self,
        uri: &str,
        now: Instant,
    ) -> Result<Arc<[u8]>, DiagnosticsReadErrorV3> {
        let (packet_id, supplied_token) = parse_capability_uri_v3(uri)?;
        let Some(index) = self
            .entries
            .iter()
            .position(|entry| entry.packet_id == packet_id)
        else {
            return Err(DiagnosticsReadErrorV3::CapabilityUnavailable);
        };
        if now >= self.entries[index].expires_at {
            let expired = self
                .entries
                .remove(index)
                .expect("entry index remains valid");
            self.retained_bytes -= expired.bytes.len();
            return Err(DiagnosticsReadErrorV3::CapabilityUnavailable);
        }
        let entry = &self.entries[index];
        if !constant_time_eq_v3(&entry.token, &supplied_token) {
            return Err(DiagnosticsReadErrorV3::CapabilityUnavailable);
        }
        Ok(Arc::clone(&entry.bytes))
    }

    #[cfg(test)]
    fn entry_count(&self) -> usize {
        self.entries.len()
    }

    #[cfg(test)]
    fn retained_bytes(&self) -> usize {
        self.retained_bytes
    }

    fn prune_expired_at(&mut self, now: Instant) {
        let mut index = 0;
        while index < self.entries.len() {
            if now >= self.entries[index].expires_at {
                let expired = self
                    .entries
                    .remove(index)
                    .expect("entry index remains valid");
                self.retained_bytes -= expired.bytes.len();
            } else {
                index += 1;
            }
        }
    }

    fn evict_oldest(&mut self) {
        if let Some(evicted) = self.entries.pop_front() {
            self.retained_bytes -= evicted.bytes.len();
        }
    }
}

pub(crate) fn attach_capability_uri_v3(
    projection: &mut Value,
    grant: &DiagnosticsGrantV3,
) -> Result<(), DiagnosticsReadErrorV3> {
    let reference = projection
        .pointer_mut("/diagnostics/reference")
        .and_then(Value::as_object_mut)
        .ok_or(DiagnosticsReadErrorV3::Internal)?;
    if reference.get("sha256").and_then(Value::as_str) != Some(grant.sha256.as_str())
        || reference.get("byte_length").and_then(Value::as_u64)
            != u64::try_from(grant.byte_length).ok()
    {
        return Err(DiagnosticsReadErrorV3::Internal);
    }
    reference.insert("uri".to_string(), Value::String(grant.uri.clone()));
    Ok(())
}

fn capability_token_v3(secret: &[u8; 32], binding: &DiagnosticsBindingV3) -> [u8; 32] {
    let mut message = Vec::new();
    message.extend_from_slice(b"codestory.packet-diagnostics.v3\0");
    for field in [
        binding.packet_id.as_bytes(),
        binding.project_id.as_bytes(),
        binding.publication_id.as_bytes(),
        binding.request_digest.as_bytes(),
    ] {
        message.extend_from_slice(&(field.len() as u64).to_be_bytes());
        message.extend_from_slice(field);
    }
    message.extend_from_slice(&binding.wall_expiry_epoch_ms.to_be_bytes());
    hmac_sha256_v3(secret, &message)
}

fn hmac_sha256_v3(key: &[u8], message: &[u8]) -> [u8; 32] {
    const BLOCK_BYTES: usize = 64;
    let mut padded = [0_u8; BLOCK_BYTES];
    if key.len() > BLOCK_BYTES {
        padded[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        padded[..key.len()].copy_from_slice(key);
    }
    let mut inner_key = [0x36_u8; BLOCK_BYTES];
    let mut outer_key = [0x5c_u8; BLOCK_BYTES];
    for index in 0..BLOCK_BYTES {
        inner_key[index] ^= padded[index];
        outer_key[index] ^= padded[index];
    }
    let mut inner = Sha256::new();
    inner.update(inner_key);
    inner.update(message);
    let inner = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(outer_key);
    outer.update(inner);
    outer.finalize().into()
}

fn parse_capability_uri_v3(uri: &str) -> Result<(String, [u8; 32]), DiagnosticsReadErrorV3> {
    let Some(path) = uri.strip_prefix("codestory://packet-diagnostics/") else {
        return Err(DiagnosticsReadErrorV3::MalformedUri);
    };
    let mut segments = path.split('/');
    let (Some(packet_id), Some(token), None) = (segments.next(), segments.next(), segments.next())
    else {
        return Err(DiagnosticsReadErrorV3::MalformedUri);
    };
    if !is_random_packet_id_v3(packet_id) {
        return Err(DiagnosticsReadErrorV3::MalformedUri);
    }
    let token = decode_hex_32_v3(token).ok_or(DiagnosticsReadErrorV3::MalformedUri)?;
    Ok((packet_id.to_string(), token))
}

fn is_random_packet_id_v3(value: &str) -> bool {
    Uuid::parse_str(value)
        .ok()
        .is_some_and(|uuid| uuid.get_version() == Some(uuid::Version::Random))
}

fn decode_hex_32_v3(value: &str) -> Option<[u8; 32]> {
    if value.len() != 64 {
        return None;
    }
    let mut decoded = [0_u8; 32];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        let high = hex_nibble_v3(pair[0])?;
        let low = hex_nibble_v3(pair[1])?;
        decoded[index] = (high << 4) | low;
    }
    Some(decoded)
}

fn hex_nibble_v3(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        _ => None,
    }
}

fn constant_time_eq_v3(left: &[u8; 32], right: &[u8; 32]) -> bool {
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

fn sha256_hex_v3(bytes: &[u8]) -> String {
    hex_v3(&Sha256::digest(bytes))
}

fn hex_v3(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use uuid::Uuid;

    use super::*;

    fn binding(packet_id: String, wall_expiry_epoch_ms: u64) -> DiagnosticsBindingV3 {
        DiagnosticsBindingV3 {
            packet_id,
            project_id: "project-1".into(),
            publication_id: "core-1/retrieval-1".into(),
            request_digest: "a".repeat(64),
            wall_expiry_epoch_ms,
        }
    }

    #[test]
    fn capability_registry_serves_exact_immutable_bytes_with_same_session_hmac() {
        assert_eq!(
            hex_v3(&hmac_sha256_v3(&[0x0b; 20], b"Hi There")),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
        let now = Instant::now();
        let mut registry = DiagnosticsRegistryV3::new_with_secret([7; 32]);
        let bytes = br#"{"kind":"complete","rows":[1,2,3]}"#.to_vec();
        let grant = registry
            .register_at(
                binding(Uuid::new_v4().to_string(), 99_000),
                bytes.clone(),
                now,
            )
            .expect("registered capability");
        assert_eq!(grant.byte_length, bytes.len());
        assert_eq!(grant.sha256.len(), 64);
        assert!(grant.uri.starts_with("codestory://packet-diagnostics/"));
        let first = registry
            .read_at(&grant.uri, now)
            .expect("same-session read");
        let second = registry
            .read_at(&grant.uri, now + Duration::from_secs(599))
            .expect("read before monotonic expiry");
        assert_eq!(&*first, bytes);
        assert!(Arc::ptr_eq(&first, &second));
    }

    #[test]
    fn capability_tamper_expiry_eviction_and_cross_session_are_uniformly_unavailable() {
        let now = Instant::now();
        let mut registry = DiagnosticsRegistryV3::new_with_secret([3; 32]);
        let first = registry
            .register_at(binding(Uuid::new_v4().to_string(), 1), vec![1], now)
            .unwrap();

        let mut tampered = first.uri.clone();
        let final_byte = tampered.pop().unwrap();
        tampered.push(if final_byte == '0' { '1' } else { '0' });
        assert_eq!(
            registry.read_at(&tampered, now),
            Err(DiagnosticsReadErrorV3::CapabilityUnavailable)
        );
        assert_eq!(
            registry.read_at(&first.uri, now + Duration::from_secs(600)),
            Err(DiagnosticsReadErrorV3::CapabilityUnavailable)
        );

        let mut newest = None;
        for index in 0..=DIAGNOSTIC_REGISTRY_MAX_ENTRIES_V3 {
            newest = Some(
                registry
                    .register_at(
                        binding(Uuid::new_v4().to_string(), index as u64),
                        vec![index as u8],
                        now,
                    )
                    .unwrap(),
            );
        }
        assert_eq!(registry.entry_count(), DIAGNOSTIC_REGISTRY_MAX_ENTRIES_V3);
        assert_eq!(
            registry.read_at(&first.uri, now),
            Err(DiagnosticsReadErrorV3::CapabilityUnavailable)
        );

        let mut other_session = DiagnosticsRegistryV3::new_with_secret([4; 32]);
        assert_eq!(
            other_session.read_at(&newest.unwrap().uri, now),
            Err(DiagnosticsReadErrorV3::CapabilityUnavailable)
        );
    }

    #[test]
    fn capability_registry_enforces_uri_and_storage_limits_without_live_reads() {
        let now = Instant::now();
        let mut registry = DiagnosticsRegistryV3::new_with_secret([9; 32]);
        assert_eq!(
            registry.read_at("codestory://packet-diagnostics/not-a-uuid/nope", now),
            Err(DiagnosticsReadErrorV3::MalformedUri)
        );
        assert_eq!(DiagnosticsReadErrorV3::MalformedUri.jsonrpc_code(), -32602);
        assert_eq!(
            DiagnosticsReadErrorV3::CapabilityUnavailable.jsonrpc_code(),
            -32002
        );
        assert_eq!(DiagnosticsReadErrorV3::Internal.jsonrpc_code(), -32603);

        let oversized = vec![0; DIAGNOSTIC_ENTRY_MAX_BYTES_V3 + 1];
        assert_eq!(
            registry.register_at(binding(Uuid::new_v4().to_string(), 1), oversized, now),
            Err(DiagnosticsReadErrorV3::Internal)
        );
        for _ in 0..DIAGNOSTIC_REGISTRY_MAX_ENTRIES_V3 {
            registry
                .register_at(
                    binding(Uuid::new_v4().to_string(), 1),
                    vec![0; DIAGNOSTIC_ENTRY_MAX_BYTES_V3],
                    now,
                )
                .unwrap();
        }
        assert_eq!(registry.retained_bytes(), DIAGNOSTIC_REGISTRY_MAX_BYTES_V3);
    }

    #[test]
    fn capability_registration_requires_random_uuid_v4_packet_identity() {
        let mut registry = DiagnosticsRegistryV3::new_with_secret([5; 32]);
        assert_eq!(
            registry.register_at(binding(Uuid::nil().to_string(), 1), vec![1], Instant::now(),),
            Err(DiagnosticsReadErrorV3::Internal)
        );
    }
}
