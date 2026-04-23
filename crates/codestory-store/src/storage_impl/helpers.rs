use crate::StorageError;
use codestory_contracts::graph::NodeId;

const STORED_VECTOR_ENCODING_ENV: &str = "CODESTORY_STORED_VECTOR_ENCODING";
const EMBEDDING_BLOB_MAGIC: &[u8; 4] = b"CSE1";
const EMBEDDING_BLOB_ENCODING_LEGACY_INT8: u8 = 1;
const EMBEDDING_BLOB_ENCODING_SCALED_INT8: u8 = 2;
const EMBEDDING_BLOB_ENCODING_COMPACT_SCALED_INT8: u8 = 3;
const EMBEDDING_BLOB_HEADER_LEN: usize = 9;
const EMBEDDING_BLOB_SCALED_INT8_HEADER_LEN: usize = 13;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EmbeddingBlobEncoding {
    Float32,
    Int8,
}

pub(crate) fn numbered_placeholders(start: usize, count: usize) -> String {
    (start..start + count)
        .map(|idx| format!("?{idx}"))
        .collect::<Vec<_>>()
        .join(",")
}

pub(crate) fn question_placeholders(count: usize) -> String {
    std::iter::repeat_n("?", count)
        .collect::<Vec<_>>()
        .join(",")
}

pub(crate) fn serialize_candidate_targets(
    candidates: &[NodeId],
) -> Result<Option<String>, StorageError> {
    if candidates.is_empty() {
        return Ok(None);
    }
    let raw: Vec<i64> = candidates.iter().map(|id| id.0).collect();
    Ok(Some(serde_json::to_string(&raw).map_err(|e| {
        StorageError::Other(format!("failed to serialize edge candidates: {e}"))
    })?))
}

pub(crate) fn deserialize_candidate_targets(
    payload: Option<&str>,
) -> Result<Vec<NodeId>, StorageError> {
    let Some(payload) = payload else {
        return Ok(Vec::new());
    };
    if payload.trim().is_empty() {
        return Ok(Vec::new());
    }
    let parsed: Vec<i64> = serde_json::from_str(payload)
        .map_err(|e| StorageError::Other(format!("failed to parse edge candidate payload: {e}")))?;
    Ok(parsed.into_iter().map(NodeId).collect())
}

pub(crate) fn encode_embedding_blob(values: &[f32]) -> Vec<u8> {
    encode_embedding_blob_with_encoding(values, embedding_blob_encoding_from_env())
}

fn encode_embedding_blob_with_encoding(values: &[f32], encoding: EmbeddingBlobEncoding) -> Vec<u8> {
    match encoding {
        EmbeddingBlobEncoding::Float32 => encode_float32_embedding_blob(values),
        EmbeddingBlobEncoding::Int8 => encode_int8_embedding_blob(values),
    }
}

fn embedding_blob_encoding_from_env() -> EmbeddingBlobEncoding {
    match std::env::var(STORED_VECTOR_ENCODING_ENV) {
        Ok(raw) if raw.trim().eq_ignore_ascii_case("int8") => EmbeddingBlobEncoding::Int8,
        _ => EmbeddingBlobEncoding::Float32,
    }
}

fn encode_float32_embedding_blob(values: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(std::mem::size_of_val(values));
    for value in values {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out
}

fn encode_int8_embedding_blob(values: &[f32]) -> Vec<u8> {
    let scale = values
        .iter()
        .map(|value| value.abs())
        .fold(0.0_f32, f32::max);
    let quantization_scale = if scale <= f32::EPSILON { 1.0 } else { scale };
    let mut out = Vec::with_capacity(EMBEDDING_BLOB_HEADER_LEN + values.len());
    out.extend_from_slice(EMBEDDING_BLOB_MAGIC);
    out.push(EMBEDDING_BLOB_ENCODING_COMPACT_SCALED_INT8);
    out.extend_from_slice(&quantization_scale.to_le_bytes());
    for value in values {
        out.push(
            (value / quantization_scale * 127.0)
                .round()
                .clamp(-127.0, 127.0) as i8 as u8,
        );
    }
    out
}

pub(crate) fn decode_embedding_blob(blob: &[u8]) -> Result<Vec<f32>, StorageError> {
    if blob.starts_with(EMBEDDING_BLOB_MAGIC) {
        return decode_versioned_embedding_blob(blob);
    }
    decode_float32_embedding_blob(blob)
}

fn decode_versioned_embedding_blob(blob: &[u8]) -> Result<Vec<f32>, StorageError> {
    if blob.len() < EMBEDDING_BLOB_HEADER_LEN {
        return Err(StorageError::Other(
            "invalid embedding blob length: incomplete versioned header".to_string(),
        ));
    }
    match blob[4] {
        EMBEDDING_BLOB_ENCODING_LEGACY_INT8 => {
            let dim = u32::from_le_bytes([blob[5], blob[6], blob[7], blob[8]]) as usize;
            let payload = &blob[EMBEDDING_BLOB_HEADER_LEN..];
            if payload.len() != dim {
                return Err(StorageError::Other(format!(
                    "invalid int8 embedding blob length: expected {dim} bytes, got {}",
                    payload.len()
                )));
            }
            let mut values = payload
                .iter()
                .map(|value| (*value as i8) as f32 / 127.0)
                .collect::<Vec<_>>();
            l2_normalize(&mut values);
            Ok(values)
        }
        EMBEDDING_BLOB_ENCODING_SCALED_INT8 => {
            if blob.len() < EMBEDDING_BLOB_SCALED_INT8_HEADER_LEN {
                return Err(StorageError::Other(
                    "invalid scaled int8 embedding blob length: incomplete header".to_string(),
                ));
            }
            let dim = u32::from_le_bytes([blob[5], blob[6], blob[7], blob[8]]) as usize;
            let scale = f32::from_le_bytes([blob[9], blob[10], blob[11], blob[12]]);
            let payload = &blob[EMBEDDING_BLOB_SCALED_INT8_HEADER_LEN..];
            if payload.len() != dim {
                return Err(StorageError::Other(format!(
                    "invalid scaled int8 embedding blob length: expected {dim} bytes, got {}",
                    payload.len()
                )));
            }
            let mut values = payload
                .iter()
                .map(|value| (*value as i8) as f32 / 127.0 * scale)
                .collect::<Vec<_>>();
            l2_normalize(&mut values);
            Ok(values)
        }
        EMBEDDING_BLOB_ENCODING_COMPACT_SCALED_INT8 => {
            let scale = f32::from_le_bytes([blob[5], blob[6], blob[7], blob[8]]);
            let payload = &blob[EMBEDDING_BLOB_HEADER_LEN..];
            if payload.is_empty() {
                return Err(StorageError::Other(
                    "invalid compact scaled int8 embedding blob length: empty payload".to_string(),
                ));
            }
            let mut values = payload
                .iter()
                .map(|value| (*value as i8) as f32 / 127.0 * scale)
                .collect::<Vec<_>>();
            l2_normalize(&mut values);
            Ok(values)
        }
        other => Err(StorageError::Other(format!(
            "unsupported embedding blob encoding tag: {other}"
        ))),
    }
}

fn decode_float32_embedding_blob(blob: &[u8]) -> Result<Vec<f32>, StorageError> {
    if !blob.len().is_multiple_of(std::mem::size_of::<f32>()) {
        return Err(StorageError::Other(
            "invalid embedding blob length: expected multiple of 4 bytes".to_string(),
        ));
    }

    let mut out = Vec::with_capacity(blob.len() / std::mem::size_of::<f32>());
    for chunk in blob.chunks_exact(std::mem::size_of::<f32>()) {
        let bytes: [u8; 4] = [chunk[0], chunk[1], chunk[2], chunk[3]];
        out.push(f32::from_le_bytes(bytes));
    }
    Ok(out)
}

fn l2_normalize(values: &mut [f32]) {
    let norm = values.iter().map(|value| value * value).sum::<f32>().sqrt();
    if norm <= f32::EPSILON {
        return;
    }
    for value in values {
        *value /= norm;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decode_legacy_float32_embedding_blob() {
        let values = [0.25, -0.5, 0.75];

        let encoded = encode_embedding_blob_with_encoding(&values, EmbeddingBlobEncoding::Float32);
        let decoded = decode_embedding_blob(&encoded).expect("decode legacy f32 blob");

        assert_eq!(encoded.len(), values.len() * std::mem::size_of::<f32>());
        assert_eq!(decoded, values);
    }

    #[test]
    fn test_int8_embedding_blob_is_compact_and_normalized() {
        let values = [0.6, -0.8, 0.0];

        let encoded = encode_embedding_blob_with_encoding(&values, EmbeddingBlobEncoding::Int8);
        let decoded = decode_embedding_blob(&encoded).expect("decode int8 blob");
        let norm = decoded
            .iter()
            .map(|value| value * value)
            .sum::<f32>()
            .sqrt();

        assert_eq!(encoded.len(), EMBEDDING_BLOB_HEADER_LEN + values.len());
        assert!((norm - 1.0).abs() < 1.0e-6);
        assert!((decoded[0] - 0.6).abs() < 0.01);
        assert!((decoded[1] + 0.8).abs() < 0.01);
    }

    #[test]
    fn test_decode_legacy_scaled_int8_embedding_blob() {
        let values = [0.6_f32, -0.8, 0.0];
        let dim = values.len() as u32;
        let scale = 0.8_f32;
        let mut encoded = Vec::with_capacity(EMBEDDING_BLOB_SCALED_INT8_HEADER_LEN + values.len());
        encoded.extend_from_slice(EMBEDDING_BLOB_MAGIC);
        encoded.push(EMBEDDING_BLOB_ENCODING_SCALED_INT8);
        encoded.extend_from_slice(&dim.to_le_bytes());
        encoded.extend_from_slice(&scale.to_le_bytes());
        for value in values {
            encoded.push((value / scale * 127.0).round() as i8 as u8);
        }

        let decoded = decode_embedding_blob(&encoded).expect("decode legacy scaled int8 blob");

        assert!((decoded[0] - 0.6).abs() < 0.01);
        assert!((decoded[1] + 0.8).abs() < 0.01);
    }
}
