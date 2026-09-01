//! Parser for the public `call-path/v1` contract grammar.
//!
//! The verifier's public input is a host-supplied text document. This module
//! is the only place that reads it.
//!
//! ```text
//! call-path/v1
//! from symbol "app::start" in "src/app.rs"
//! direct-call symbol "service::load" in "src/service.rs"
//! direct-call canonical "store::read"
//! prohibit-through symbol "legacy::shim"
//! exclude-from-projection symbol "tracing::span"
//! ```

use codestory_runtime::proof_qualification_support as proof;

pub(crate) const CALL_PATH_GRAMMAR_HEADER: &str = "call-path/v1";
const MAX_QUOTED_ATOM_BYTES: usize = 512;
const MAX_DIRECT_CALLS: usize = 6;
const MAX_SCOPE_SELECTORS: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CallPathSyntaxError {
    pub(crate) message: String,
}

impl std::fmt::Display for CallPathSyntaxError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

fn syntax_error(message: impl Into<String>) -> CallPathSyntaxError {
    CallPathSyntaxError {
        message: message.into(),
    }
}

pub(crate) fn parse_call_path_document(
    document: &str,
) -> Result<proof::UnvalidatedCallPathContract, CallPathSyntaxError> {
    let (header, body_offset) = split_header(document)?;
    if header != CALL_PATH_GRAMMAR_HEADER {
        return Err(syntax_error(format!(
            "call_path must begin with the line `{CALL_PATH_GRAMMAR_HEADER}`, found `{header}`"
        )));
    }
    Parser::new(&document[body_offset..]).parse()
}

fn split_header(document: &str) -> Result<(&str, usize), CallPathSyntaxError> {
    let mut offset = 0usize;
    for line in document.split_inclusive('\n') {
        let trimmed = line.trim();
        offset += line.len();
        if !trimmed.is_empty() {
            return Ok((trimmed, offset));
        }
    }
    Err(syntax_error(format!(
        "call_path is empty; it must begin with the line `{CALL_PATH_GRAMMAR_HEADER}`"
    )))
}

struct Parser<'a> {
    body: &'a str,
    clauses: Vec<proof::ClauseAnchor>,
    start: Option<proof::UnvalidatedExactSymbolSelector>,
    steps: Vec<proof::UnvalidatedDirectCallStep>,
    prohibit_traversal_through: Vec<proof::UnvalidatedExactScopeSelector>,
    exclude_from_projection: Vec<proof::UnvalidatedExactScopeSelector>,
    clause_sequence: usize,
    saw_from_line: bool,
    saw_direct_call_line: bool,
}

impl<'a> Parser<'a> {
    fn new(body: &'a str) -> Self {
        Self {
            body,
            clauses: Vec::new(),
            start: None,
            steps: Vec::new(),
            prohibit_traversal_through: Vec::new(),
            exclude_from_projection: Vec::new(),
            clause_sequence: 0,
            saw_from_line: false,
            saw_direct_call_line: false,
        }
    }

    fn parse(mut self) -> Result<proof::UnvalidatedCallPathContract, CallPathSyntaxError> {
        let mut offset = 0usize;
        for raw_line in self.body.split_inclusive('\n') {
            let line_start = offset;
            offset += raw_line.len();
            let trimmed = raw_line.trim_end_matches(['\n', '\r']);
            let leading = trimmed.len() - trimmed.trim_start().len();
            let content = trimmed.trim();
            if content.is_empty() {
                continue;
            }
            self.parse_line(line_start + leading, content);
        }

        let Some(start) = self.start else {
            return Err(syntax_error(if self.saw_from_line {
                "the `from` line does not name a public start selector the grammar accepts; \
                 internal node identities cannot be a `from` selector. \
                 write `from symbol \"<qualified-name>\" [in \"<path>\"]` or `from canonical \"<id>\"`"
            } else {
                "call_path must declare one `from` line"
            }));
        };
        if self.steps.is_empty() {
            return Err(syntax_error(if self.saw_direct_call_line {
                "no `direct-call` line could be read; write `direct-call symbol \"<name>\"` \
                 or `direct-call canonical \"<id>\"`"
            } else {
                "call_path must declare at least one `direct-call` line"
            }));
        }
        if self.steps.len() > MAX_DIRECT_CALLS {
            return Err(syntax_error(format!(
                "call_path allows at most {MAX_DIRECT_CALLS} `direct-call` lines"
            )));
        }
        if self.prohibit_traversal_through.len() > MAX_SCOPE_SELECTORS
            || self.exclude_from_projection.len() > MAX_SCOPE_SELECTORS
        {
            return Err(syntax_error(format!(
                "call_path allows at most {MAX_SCOPE_SELECTORS} prohibit-through and {MAX_SCOPE_SELECTORS} exclude-from-projection lines"
            )));
        }
        Ok(proof::UnvalidatedCallPathContract::new(
            self.body,
            self.clauses,
            proof::UnvalidatedCallPathSpec {
                start,
                steps: self.steps,
                prohibit_traversal_through: self.prohibit_traversal_through,
                exclude_from_projection: self.exclude_from_projection,
            },
        ))
    }

    fn parse_line(&mut self, line_start: usize, content: &str) {
        let lower = content.to_ascii_lowercase();
        if lower.starts_with("from ") {
            self.parse_from(line_start, content);
        } else if lower.starts_with("direct-call ") {
            self.parse_direct_call(line_start, content);
        } else if lower.starts_with("prohibit-through ") {
            self.parse_scope(line_start, content, ScopeKind::ProhibitTraversal);
        } else if lower.starts_with("exclude-from-projection ") {
            self.parse_scope(line_start, content, ScopeKind::ExcludeFromProjection);
        } else {
            self.unresolved(
                line_start,
                content,
                proof::UnresolvedMaterialReason::UnsupportedInterpretation,
            );
        }
    }

    fn parse_from(&mut self, line_start: usize, content: &str) {
        self.saw_from_line = true;
        if self.start.is_some() {
            self.unresolved(
                line_start,
                content,
                proof::UnresolvedMaterialReason::AmbiguousSelectorResolution,
            );
            return;
        }
        match parse_selector_directive(content, "from") {
            Some(parsed) => {
                self.resolved(line_start, content, &[proof::ProofContractField::Start]);
                self.start = Some(parsed.into_symbol());
            }
            None => {
                self.unresolved(
                    line_start,
                    content,
                    proof::UnresolvedMaterialReason::MissingSelectorResolution,
                );
            }
        }
    }

    fn parse_direct_call(&mut self, line_start: usize, content: &str) {
        self.saw_direct_call_line = true;
        let Ok(step) = u8::try_from(self.steps.len()) else {
            self.unresolved(
                line_start,
                content,
                proof::UnresolvedMaterialReason::UnsupportedInterpretation,
            );
            return;
        };
        match parse_selector_directive(content, "direct-call") {
            Some(parsed) => {
                self.resolved(
                    line_start,
                    content,
                    &[
                        proof::ProofContractField::Ordering { step },
                        proof::ProofContractField::Directness { step },
                        proof::ProofContractField::Relation { step },
                        proof::ProofContractField::StepTarget { step },
                    ],
                );
                self.steps.push(proof::UnvalidatedDirectCallStep {
                    target: parsed.into_symbol(),
                });
            }
            None => {
                self.unresolved(
                    line_start,
                    content,
                    proof::UnresolvedMaterialReason::MissingSelectorResolution,
                );
            }
        }
    }

    fn parse_scope(&mut self, line_start: usize, content: &str, kind: ScopeKind) {
        let prefix = match kind {
            ScopeKind::ProhibitTraversal => "prohibit-through",
            ScopeKind::ExcludeFromProjection => "exclude-from-projection",
        };
        let position = match kind {
            ScopeKind::ProhibitTraversal => self.prohibit_traversal_through.len(),
            ScopeKind::ExcludeFromProjection => self.exclude_from_projection.len(),
        };
        let Ok(index) = u8::try_from(position) else {
            self.unresolved(
                line_start,
                content,
                proof::UnresolvedMaterialReason::UnsupportedInterpretation,
            );
            return;
        };
        match parse_selector_directive(content, prefix) {
            Some(parsed) => {
                let field = match kind {
                    ScopeKind::ProhibitTraversal => {
                        proof::ProofContractField::TraversalProhibition { index }
                    }
                    ScopeKind::ExcludeFromProjection => {
                        proof::ProofContractField::ProjectionExclusion { index }
                    }
                };
                self.resolved(line_start, content, &[field]);
                match kind {
                    ScopeKind::ProhibitTraversal => {
                        self.prohibit_traversal_through.push(parsed.into_scope())
                    }
                    ScopeKind::ExcludeFromProjection => {
                        self.exclude_from_projection.push(parsed.into_scope())
                    }
                }
            }
            None => self.unresolved(
                line_start,
                content,
                proof::UnresolvedMaterialReason::MissingSelectorResolution,
            ),
        }
    }

    fn resolved(&mut self, start: usize, quote: &str, fields: &[proof::ProofContractField]) {
        self.push(
            start,
            quote,
            proof::ClauseClassification::ResolvedMaterial {
                fields: fields.to_vec(),
            },
        );
    }

    fn unresolved(&mut self, start: usize, quote: &str, reason: proof::UnresolvedMaterialReason) {
        self.push(
            start,
            quote,
            proof::ClauseClassification::UnresolvedMaterial { reason },
        );
    }

    fn push(&mut self, start: usize, quote: &str, classification: proof::ClauseClassification) {
        debug_assert_eq!(
            self.body.get(start..start + quote.len()),
            Some(quote),
            "clause anchors must quote the body exactly"
        );
        self.clause_sequence += 1;
        self.clauses.push(proof::ClauseAnchor {
            clause_id: format!("clause-{}", self.clause_sequence),
            start,
            end: start + quote.len(),
            quote: quote.to_owned(),
            classification,
        });
    }
}

#[derive(Debug, Clone, Copy)]
enum ScopeKind {
    ProhibitTraversal,
    ExcludeFromProjection,
}

struct ParsedSelector {
    kind: SelectorKind,
}

enum SelectorKind {
    Canonical(String),
    QualifiedName {
        qualified_name: String,
        project_file_components: Option<Vec<String>>,
    },
}

impl ParsedSelector {
    fn into_symbol(self) -> proof::UnvalidatedExactSymbolSelector {
        match self.kind {
            SelectorKind::Canonical(id) => proof::UnvalidatedExactSymbolSelector::CanonicalId(id),
            SelectorKind::QualifiedName {
                qualified_name,
                project_file_components,
            } => proof::UnvalidatedExactSymbolSelector::QualifiedName {
                qualified_name,
                project_file_components,
            },
        }
    }

    fn into_scope(self) -> proof::UnvalidatedExactScopeSelector {
        match self.kind {
            SelectorKind::Canonical(id) => proof::UnvalidatedExactScopeSelector::CanonicalId(id),
            SelectorKind::QualifiedName {
                qualified_name,
                project_file_components,
            } => proof::UnvalidatedExactScopeSelector::QualifiedName {
                qualified_name,
                project_file_components,
            },
        }
    }
}

fn parse_selector_directive(content: &str, prefix: &str) -> Option<ParsedSelector> {
    let rest = content.get(prefix.len()..)?.trim_start();
    let (kind, rest) = rest.split_once(char::is_whitespace)?;
    let rest = rest.trim_start();
    match kind {
        "canonical" => {
            let (id, leftover) = parse_quoted(rest)?;
            if !leftover.trim().is_empty() || !canonical_id_ok(&id) {
                return None;
            }
            Some(ParsedSelector {
                kind: SelectorKind::Canonical(id),
            })
        }
        "symbol" => {
            let (name, rest) = parse_quoted(rest)?;
            let rest = rest.trim_start();
            let path = if rest.is_empty() {
                None
            } else {
                let (in_kw, rest) = rest.split_once(char::is_whitespace)?;
                if !in_kw.eq_ignore_ascii_case("in") {
                    return None;
                }
                let (path, leftover) = parse_quoted(rest.trim_start())?;
                if !leftover.trim().is_empty() {
                    return None;
                }
                Some(path_components(&path)?)
            };
            if !symbol_name_ok(&name) {
                return None;
            }
            Some(ParsedSelector {
                kind: SelectorKind::QualifiedName {
                    qualified_name: name,
                    project_file_components: path,
                },
            })
        }
        _ => None,
    }
}

fn parse_quoted(input: &str) -> Option<(String, &str)> {
    let input = input.trim_start();
    if !input.starts_with('"') {
        return None;
    }
    let mut output = String::new();
    let mut output_bytes = 0usize;
    let mut chars = input[1..].char_indices();
    while let Some((relative, ch)) = chars.next() {
        match ch {
            '"' => {
                if output_bytes > MAX_QUOTED_ATOM_BYTES {
                    return None;
                }
                return Some((output, &input[relative + 2..]));
            }
            '\\' => {
                let escaped = chars.next()?.1;
                let mapped = match escaped {
                    '"' | '\\' | '/' => escaped,
                    'n' => '\n',
                    'r' => '\r',
                    't' => '\t',
                    _ => return None,
                };
                output_bytes = output_bytes.saturating_add(mapped.len_utf8());
                if output_bytes > MAX_QUOTED_ATOM_BYTES {
                    return None;
                }
                output.push(mapped);
            }
            _ => {
                output_bytes = output_bytes.saturating_add(ch.len_utf8());
                if output_bytes > MAX_QUOTED_ATOM_BYTES {
                    return None;
                }
                output.push(ch);
            }
        }
    }
    None
}

fn symbol_name_ok(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_QUOTED_ATOM_BYTES
        && !name.chars().any(|character| {
            character.is_whitespace()
                || matches!(
                    character,
                    '(' | ')' | '*' | '?' | '"' | '\0' | '{' | '}' | '[' | ']'
                )
        })
}

fn canonical_id_ok(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_QUOTED_ATOM_BYTES
        && !id.contains("..")
        && !id.starts_with('/')
        && !id.contains('\\')
}

fn path_components(path: &str) -> Option<Vec<String>> {
    if path.is_empty()
        || path.len() > MAX_QUOTED_ATOM_BYTES
        || path.starts_with('/')
        || path.contains('\\')
        || path.contains(':')
        || path.starts_with('~')
    {
        return None;
    }
    let components = path.split('/').map(str::to_owned).collect::<Vec<_>>();
    if components
        .iter()
        .any(|component| component.is_empty() || component == "." || component == "..")
    {
        return None;
    }
    Some(components)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOCUMENT: &str = concat!(
        "call-path/v1\n",
        "from symbol \"crate::module::Alpha\"\n",
        "direct-call symbol \"crate::module::Beta\"\n",
        "direct-call symbol \"Gamma\" in \"src/gamma.rs\"\n",
        "prohibit-through symbol \"crate::detail::Helper\"\n",
        "exclude-from-projection symbol \"crate::test_support\"\n",
    );

    fn validated(document: &str) -> proof::ValidationOutcome {
        let contract = parse_call_path_document(document).expect("parse");
        proof::validate_contract(contract).expect("validate")
    }

    #[test]
    fn the_frozen_grammar_validates_with_no_translation_gaps() {
        match validated(DOCUMENT) {
            proof::ValidationOutcome::Validated { .. } => {}
            proof::ValidationOutcome::Unknown { gaps, .. } => {
                panic!("the frozen example must validate cleanly, found gaps {gaps:?}")
            }
        }
    }

    #[test]
    fn every_non_whitespace_byte_of_the_body_is_classified() {
        let contract = parse_call_path_document(DOCUMENT).expect("parse");
        let body = contract.source_text().to_owned();
        let mut covered = vec![false; body.len()];
        for clause in contract.clauses() {
            assert_eq!(&body[clause.start..clause.end], clause.quote);
            for byte in &mut covered[clause.start..clause.end] {
                *byte = true;
            }
        }
        for (offset, character) in body.char_indices() {
            if character.is_whitespace() {
                continue;
            }
            assert!(
                covered[offset],
                "byte {offset} ({character:?}) is unclassified"
            );
        }
    }

    #[test]
    fn a_path_qualified_selector_carries_its_file_components() {
        let contract = parse_call_path_document(DOCUMENT).expect("parse");
        let target = match &contract.spec().steps[1].target {
            proof::UnvalidatedExactSymbolSelector::QualifiedName {
                qualified_name,
                project_file_components,
            } => (qualified_name.clone(), project_file_components.clone()),
            other => panic!("expected a qualified name, got {other:?}"),
        };
        assert_eq!(
            target,
            (
                "Gamma".to_owned(),
                Some(vec!["src".to_owned(), "gamma.rs".to_owned()])
            )
        );
    }

    #[test]
    fn quoted_selectors_preserve_utf8_atoms_and_paths() {
        let document = concat!(
            "call-path/v1\n",
            "from symbol \"café::démarrage\" in \"src/café.rs\"\n",
            "direct-call symbol \"服务::加载\"\n",
        );
        let contract = parse_call_path_document(document).expect("parse utf-8 selectors");
        match &contract.spec().start {
            proof::UnvalidatedExactSymbolSelector::QualifiedName {
                qualified_name,
                project_file_components,
            } => {
                assert_eq!(qualified_name, "café::démarrage");
                assert_eq!(
                    project_file_components.as_deref(),
                    Some(["src".to_owned(), "café.rs".to_owned()].as_slice())
                );
            }
            other => panic!("expected a qualified utf-8 start, got {other:?}"),
        }
        match &contract.spec().steps[0].target {
            proof::UnvalidatedExactSymbolSelector::QualifiedName { qualified_name, .. } => {
                assert_eq!(qualified_name, "服务::加载")
            }
            other => panic!("expected a qualified utf-8 step, got {other:?}"),
        }
    }

    #[test]
    fn legacy_start_grammar_does_not_build_a_contract() {
        assert!(
            parse_call_path_document(
                "call-path/v1\nstart: crate::A\nstep 1: direct call -> crate::B\n"
            )
            .is_err()
        );
    }

    #[test]
    fn a_missing_or_wrong_version_line_is_a_syntax_error() {
        assert!(parse_call_path_document("").is_err());
        assert!(parse_call_path_document("from symbol \"A\"\ndirect-call symbol \"B\"\n").is_err());
        assert!(parse_call_path_document("call-path/v2\nfrom symbol \"A\"\n").is_err());
    }

    #[test]
    fn a_document_without_from_or_direct_call_is_a_syntax_error() {
        assert!(
            parse_call_path_document("call-path/v1\ndirect-call symbol \"B\"\n").is_err(),
            "a contract with no from cannot be built"
        );
        assert!(
            parse_call_path_document("call-path/v1\nfrom symbol \"A\"\n").is_err(),
            "a contract with no direct-call cannot be built"
        );
    }

    #[test]
    fn prose_becomes_unresolved_material_not_a_silent_skip() {
        let document = concat!(
            "call-path/v1\n",
            "from symbol \"crate::module::Alpha\"\n",
            "direct-call symbol \"crate::module::Beta\"\n",
            "also please check crate::module::Delta\n",
        );
        let contract = parse_call_path_document(document).expect("parse");
        assert!(
            contract.clauses().iter().any(|clause| matches!(
                clause.classification,
                proof::ClauseClassification::UnresolvedMaterial { .. }
            )),
            "the extra line must be anchored as unresolved material"
        );
        match proof::validate_contract(contract).expect("validate") {
            proof::ValidationOutcome::Unknown { gaps, .. } => assert!(!gaps.is_empty()),
            proof::ValidationOutcome::Validated { .. } => {
                panic!("an uninterpretable line must not validate as a complete translation")
            }
        }
    }

    #[test]
    fn absolute_and_parent_paths_are_rejected() {
        for path in ["/abs/app.rs", "src/../app.rs"] {
            let document = format!(
                "call-path/v1\nfrom symbol \"A\" in \"{path}\"\ndirect-call symbol \"B\"\n"
            );
            let contract = parse_call_path_document(&document);
            match contract {
                Err(_) => {}
                Ok(parsed) => match proof::validate_contract(parsed).expect("validate") {
                    proof::ValidationOutcome::Unknown { .. } => {}
                    proof::ValidationOutcome::Validated { .. } => {
                        panic!("{path} must not validate")
                    }
                },
            }
        }
    }

    #[test]
    fn canonical_selectors_are_constructed() {
        let document = concat!(
            "call-path/v1\n",
            "from canonical \"store::read\"\n",
            "direct-call canonical \"store::write\"\n",
        );
        let contract = parse_call_path_document(document).expect("parse");
        match &contract.spec().start {
            proof::UnvalidatedExactSymbolSelector::CanonicalId(id) => {
                assert_eq!(id, "store::read");
            }
            other => panic!("expected canonical id, got {other:?}"),
        }
    }

    #[test]
    fn quoted_atoms_are_capped_at_512_bytes() {
        let huge = "a".repeat(513);
        let document = format!("call-path/v1\nfrom symbol \"{huge}\"\ndirect-call symbol \"B\"\n");
        assert!(parse_call_path_document(&document).is_err());
    }

    #[test]
    fn blank_lines_and_indentation_are_accepted() {
        let document = concat!(
            "call-path/v1\n",
            "\n",
            "  from symbol \"crate::module::Alpha\"\n",
            "\n",
            "\tdirect-call symbol \"crate::module::Beta\"\n",
        );
        match validated(document) {
            proof::ValidationOutcome::Validated { .. } => {}
            proof::ValidationOutcome::Unknown { gaps, .. } => panic!("unexpected gaps {gaps:?}"),
        }
    }

    #[test]
    fn crlf_line_endings_parse_the_same_way() {
        let document =
            "call-path/v1\r\nfrom symbol \"crate::A\"\r\ndirect-call symbol \"crate::B\"\r\n";
        match validated(document) {
            proof::ValidationOutcome::Validated { .. } => {}
            proof::ValidationOutcome::Unknown { gaps, .. } => panic!("unexpected gaps {gaps:?}"),
        }
    }

    #[test]
    fn a_second_from_line_is_ambiguous_rather_than_overwriting_the_first() {
        let document = concat!(
            "call-path/v1\n",
            "from symbol \"crate::module::Alpha\"\n",
            "from symbol \"crate::module::Other\"\n",
            "direct-call symbol \"crate::module::Beta\"\n",
        );
        let contract = parse_call_path_document(document).expect("parse");
        match &contract.spec().start {
            proof::UnvalidatedExactSymbolSelector::QualifiedName { qualified_name, .. } => {
                assert_eq!(qualified_name, "crate::module::Alpha");
            }
            other => panic!("expected a qualified name, got {other:?}"),
        }
        match proof::validate_contract(contract).expect("validate") {
            proof::ValidationOutcome::Unknown { .. } => {}
            proof::ValidationOutcome::Validated { .. } => {
                panic!("a second from must not validate as a complete translation")
            }
        }
    }
}
