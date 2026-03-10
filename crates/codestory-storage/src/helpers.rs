use crate::StorageError;
use codestory_core::NodeId;

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
    let mut out = Vec::with_capacity(std::mem::size_of_val(values));
    for value in values {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out
}

pub(crate) fn decode_embedding_blob(blob: &[u8]) -> Result<Vec<f32>, StorageError> {
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
