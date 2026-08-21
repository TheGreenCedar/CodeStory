#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum McpRevisionV3 {
    November2024,
    March2025,
    June2025,
    November2025,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BatchPolicyV3 {
    AcceptIndependentOrdered,
    Reject,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RevisionProfileV3 {
    pub(crate) revision: McpRevisionV3,
    pub(crate) tool_fields: &'static [&'static str],
    pub(crate) structured_content: bool,
    pub(crate) batch_policy: BatchPolicyV3,
}

impl McpRevisionV3 {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::November2024 => "2024-11-05",
            Self::March2025 => "2025-03-26",
            Self::June2025 => "2025-06-18",
            Self::November2025 => "2025-11-25",
        }
    }

    pub(crate) const fn all() -> &'static [Self] {
        &[
            Self::November2024,
            Self::March2025,
            Self::June2025,
            Self::November2025,
        ]
    }

    pub(crate) const fn preferred() -> Self {
        Self::November2025
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "2024-11-05" => Some(Self::November2024),
            "2025-03-26" => Some(Self::March2025),
            "2025-06-18" => Some(Self::June2025),
            "2025-11-25" => Some(Self::November2025),
            _ => None,
        }
    }

    pub(crate) const fn profile(self) -> RevisionProfileV3 {
        match self {
            Self::November2024 => RevisionProfileV3 {
                revision: self,
                tool_fields: &["name", "description", "inputSchema"],
                structured_content: false,
                batch_policy: BatchPolicyV3::AcceptIndependentOrdered,
            },
            Self::March2025 => RevisionProfileV3 {
                revision: self,
                tool_fields: &["name", "description", "inputSchema", "annotations"],
                structured_content: false,
                batch_policy: BatchPolicyV3::AcceptIndependentOrdered,
            },
            Self::June2025 | Self::November2025 => RevisionProfileV3 {
                revision: self,
                tool_fields: &[
                    "name",
                    "title",
                    "description",
                    "inputSchema",
                    "outputSchema",
                    "_meta",
                ],
                structured_content: true,
                batch_policy: BatchPolicyV3::Reject,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn revision_profiles_are_session_scoped_native_and_prefer_newest() {
        assert_eq!(
            McpRevisionV3::all()
                .iter()
                .map(|revision| revision.as_str())
                .collect::<Vec<_>>(),
            ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"]
        );
        assert_eq!(McpRevisionV3::preferred(), McpRevisionV3::November2025);
        for revision in McpRevisionV3::all() {
            assert_eq!(McpRevisionV3::parse(revision.as_str()), Some(*revision));
            assert_eq!(revision.profile().revision, *revision);
        }
        assert_eq!(McpRevisionV3::parse("2026-01-01"), None);
    }

    #[test]
    fn revision_profiles_pin_native_tool_result_and_batch_shapes() {
        let cases = [
            (
                McpRevisionV3::November2024,
                vec!["name", "description", "inputSchema"],
                false,
                BatchPolicyV3::AcceptIndependentOrdered,
            ),
            (
                McpRevisionV3::March2025,
                vec!["name", "description", "inputSchema", "annotations"],
                false,
                BatchPolicyV3::AcceptIndependentOrdered,
            ),
            (
                McpRevisionV3::June2025,
                vec![
                    "name",
                    "title",
                    "description",
                    "inputSchema",
                    "outputSchema",
                    "_meta",
                ],
                true,
                BatchPolicyV3::Reject,
            ),
            (
                McpRevisionV3::November2025,
                vec![
                    "name",
                    "title",
                    "description",
                    "inputSchema",
                    "outputSchema",
                    "_meta",
                ],
                true,
                BatchPolicyV3::Reject,
            ),
        ];
        for (revision, fields, structured, batch) in cases {
            let profile = revision.profile();
            assert_eq!(profile.tool_fields, fields);
            assert_eq!(profile.structured_content, structured);
            assert_eq!(profile.batch_policy, batch);
        }
    }
}
