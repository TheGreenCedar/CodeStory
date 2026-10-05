//! Query-origin lexical admission, independent of source/publication proof.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum LexicalAdmissionPolicy {
    #[default]
    StrictLiteralV1,
    BroadQuestionV1,
}

impl LexicalAdmissionPolicy {
    /// Preserve the strict namespace; broad executions cannot reuse it.
    pub(crate) fn cache_fingerprint(self, fingerprint: String) -> String {
        match self {
            Self::StrictLiteralV1 => fingerprint,
            Self::BroadQuestionV1 => format!("lexical-broad-question/v1:{fingerprint}"),
        }
    }
}
