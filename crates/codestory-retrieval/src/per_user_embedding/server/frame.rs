//! Incremental bounded protocol frame decoding.

use super::super::{
    EmbeddingServerStream, PER_USER_EMBEDDING_MAX_METADATA_BYTES,
    PER_USER_EMBEDDING_MAX_PAYLOAD_BYTES,
};
use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use std::io;

#[derive(Default)]
pub(in crate::per_user_embedding) struct IncrementalProtocolFrameReader {
    bytes: Vec<u8>,
}

pub(in crate::per_user_embedding) enum ProtocolFramePoll<T> {
    Pending,
    Closed,
    Ready((T, Vec<u8>)),
}

impl IncrementalProtocolFrameReader {
    pub(in crate::per_user_embedding) fn poll<T: for<'de> Deserialize<'de>>(
        &mut self,
        stream: &mut dyn EmbeddingServerStream,
    ) -> Result<ProtocolFramePoll<T>> {
        if let Some(frame) = self.decode_ready()? {
            return Ok(ProtocolFramePoll::Ready(frame));
        }
        let mut chunk = [0_u8; 8 * 1024];
        match stream.read(&mut chunk) {
            Ok(0) if self.bytes.is_empty() => return Ok(ProtocolFramePoll::Closed),
            Ok(0) => bail!("embedding_server_frame_truncated"),
            Ok(read) => self.bytes.extend_from_slice(&chunk[..read]),
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                ) =>
            {
                return Ok(ProtocolFramePoll::Pending);
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::BrokenPipe
                        | io::ErrorKind::ConnectionAborted
                        | io::ErrorKind::ConnectionReset
                        | io::ErrorKind::NotConnected
                        | io::ErrorKind::UnexpectedEof
                ) =>
            {
                return Ok(ProtocolFramePoll::Closed);
            }
            Err(error) => return Err(error).context("read held embedding lease frame"),
        }
        Ok(match self.decode_ready()? {
            Some(frame) => ProtocolFramePoll::Ready(frame),
            None => ProtocolFramePoll::Pending,
        })
    }

    fn decode_ready<T: for<'de> Deserialize<'de>>(&mut self) -> Result<Option<(T, Vec<u8>)>> {
        if self.bytes.len() < 8 {
            return Ok(None);
        }
        let control_len =
            u32::from_be_bytes(self.bytes[0..4].try_into().expect("four-byte frame length"))
                as usize;
        let payload_len =
            u32::from_be_bytes(self.bytes[4..8].try_into().expect("four-byte frame length"))
                as usize;
        if control_len == 0
            || control_len > PER_USER_EMBEDDING_MAX_METADATA_BYTES
            || payload_len > PER_USER_EMBEDDING_MAX_PAYLOAD_BYTES
        {
            bail!("embedding_server_frame_too_large");
        }
        let frame_len = 8_usize
            .checked_add(control_len)
            .and_then(|length| length.checked_add(payload_len))
            .ok_or_else(|| anyhow!("embedding_server_frame_length_overflow"))?;
        if self.bytes.len() < frame_len {
            return Ok(None);
        }
        let control = serde_json::from_slice(&self.bytes[8..8 + control_len])
            .context("decode held embedding lease control frame")?;
        let payload = self.bytes[8 + control_len..frame_len].to_vec();
        self.bytes.drain(..frame_len);
        Ok(Some((control, payload)))
    }
}
