//! Internal source addresses. These are not additions to the public packet API.
//!
//! Coordinates identify evidence; they do not assert a lexical match for a
//! symbol, graph endpoint, or file-only discovery. Runtime authenticates ranges
//! against the indexed content digest while holding the publication pin.

use crate::packet_projection_v3::Sha256DigestV3Dto;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum EvidenceAddressError {
    #[error("expected a nonempty zero-based half-open byte range")]
    ByteRange,
    #[error("expected a nonempty one-based inclusive line range")]
    LineRange,
    #[error("expected a normalized project-relative path")]
    Path,
    #[error("expected a nonempty stable identity")]
    Identity,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawByteRange {
    start: u64,
    end: u64,
}

/// Zero-based, half-open UTF-8 byte offsets. Runtime checks character boundaries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawByteRange")]
pub struct ByteRangeV1 {
    start: u64,
    end: u64,
}

impl ByteRangeV1 {
    pub fn new(start: u64, end: u64) -> Result<Self, EvidenceAddressError> {
        if start >= end {
            return Err(EvidenceAddressError::ByteRange);
        }
        Ok(Self { start, end })
    }

    pub fn start(self) -> u64 {
        self.start
    }

    pub fn end(self) -> u64 {
        self.end
    }
}

impl TryFrom<RawByteRange> for ByteRangeV1 {
    type Error = EvidenceAddressError;

    fn try_from(value: RawByteRange) -> Result<Self, Self::Error> {
        Self::new(value.start, value.end)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLineRange {
    start: u32,
    end: u32,
}

/// One-based, inclusive source line numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawLineRange")]
pub struct LineRangeV1 {
    start: u32,
    end: u32,
}

impl LineRangeV1 {
    pub fn new(start: u32, end: u32) -> Result<Self, EvidenceAddressError> {
        if start == 0 || start > end {
            return Err(EvidenceAddressError::LineRange);
        }
        Ok(Self { start, end })
    }

    pub fn start(self) -> u32 {
        self.start
    }

    pub fn end(self) -> u32 {
        self.end
    }
}

impl TryFrom<RawLineRange> for LineRangeV1 {
    type Error = EvidenceAddressError;

    fn try_from(value: RawLineRange) -> Result<Self, Self::Error> {
        Self::new(value.start, value.end)
    }
}

/// Full project-relative identity, with forward slashes on every host.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ProjectRelativePath(String);

impl ProjectRelativePath {
    pub fn new(value: impl Into<String>) -> Result<Self, EvidenceAddressError> {
        let value = value.into();
        if value
            .chars()
            .any(|character| character.is_control() || character == '\\')
            || (value
                .as_bytes()
                .first()
                .is_some_and(u8::is_ascii_alphabetic)
                && value.as_bytes().get(1) == Some(&b':'))
            || value.split('/').any(|part| matches!(part, "" | "." | ".."))
        {
            return Err(EvidenceAddressError::Path);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ProjectRelativePath {
    type Error = EvidenceAddressError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<ProjectRelativePath> for String {
    fn from(value: ProjectRelativePath) -> Self {
        value.0
    }
}

// Distinct types prevent a relation identity from being resolved as a node.
macro_rules! stable_identity {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, EvidenceAddressError> {
                let value = value.into();
                if value.trim().is_empty() || value.chars().any(char::is_control) {
                    return Err(EvidenceAddressError::Identity);
                }
                Ok(Self(value))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl TryFrom<String> for $name {
            type Error = EvidenceAddressError;

            fn try_from(value: String) -> Result<Self, Self::Error> {
                Self::new(value)
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.0
            }
        }
    };
}

stable_identity!(StableNodeId);
stable_identity!(StableRelationId);

/// The digest covers the entire source file, not just the selected range.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceRangeV1 {
    pub path: ProjectRelativePath,
    pub byte_range: ByteRangeV1,
    pub line_range: LineRangeV1,
    pub content_digest: Sha256DigestV3Dto,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum EvidenceAnchorV1 {
    Match {
        byte_range: ByteRangeV1,
        line_range: LineRangeV1,
    },
    IndexedNode {
        node_id: StableNodeId,
        source_range: SourceRangeV1,
    },
    RelationOccurrence {
        relation_id: StableRelationId,
        source_range: SourceRangeV1,
    },
    PathOnly {
        path: ProjectRelativePath,
    },
}
