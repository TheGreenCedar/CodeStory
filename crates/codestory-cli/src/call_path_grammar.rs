//! Parser for the public `call-path/v1` contract grammar.
//!
//! The verifier's public input is a text document, not a translated JSON
//! structure. This module is the only place that reads it, and it produces both
//! the contract and the clause anchors the guard checks, so no caller can hand
//! the kernel a classification the text does not support.
//!
//! ```text
//! call-path/v1
//! start: crate::module::Alpha
//! step 1: direct call -> crate::module::Beta
//! step 2: direct call -> "src/gamma.rs"::Gamma
//! prohibit traversal through: crate::detail::Helper
//! exclude from projection: crate::test_support
//! ```
//!
//! The first line names the grammar. Everything after it is the contract text
//! the clauses anchor into, so `source_text` is the document body and the
//! version line frames it rather than appearing inside it.
//!
//! A line the parser cannot interpret is not skipped. It becomes an unresolved
//! material clause, which makes the whole verification report
//! `graph_disposition: "unknown"` instead of quietly proving a smaller contract
//! than the caller wrote.

use codestory_runtime::proof_qualification_support as proof;

/// The only grammar version this build accepts.
pub(crate) const CALL_PATH_GRAMMAR_HEADER: &str = "call-path/v1";

/// A document that cannot yield a contract at all. Anything the parser can
/// represent as an unresolved clause is reported through the verification
/// result instead of here.
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

/// Split the version line from the body, returning the trimmed header and the
/// byte offset where the body begins.
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
    /// Separates "the caller never wrote this line" from "the line is there but
    /// the grammar could not read it", which are different mistakes to report.
    saw_start_line: bool,
    saw_step_line: bool,
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
            saw_start_line: false,
            saw_step_line: false,
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
            return Err(syntax_error(if self.saw_start_line {
                "the `start:` line does not name a selector the grammar accepts; \
                 write `start: <qualified::name>` or `start: \"path/to/file\"::<name>`"
            } else {
                "call_path must declare one `start: <selector>` line"
            }));
        };
        if self.steps.is_empty() {
            return Err(syntax_error(if self.saw_step_line {
                "no `step` line could be read; write `step 1: direct call -> <selector>` \
                 and number the steps consecutively from 1"
            } else {
                "call_path must declare at least one `step 1: direct call -> <selector>` line"
            }));
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
        let Some(line) = Line::split(line_start, content) else {
            self.unresolved(
                line_start,
                content,
                proof::UnresolvedMaterialReason::UnsupportedInterpretation,
            );
            return;
        };
        match line.normalized_label.as_str() {
            "start" => self.parse_start(&line),
            label if label.starts_with("step ") => self.parse_step(&line),
            "prohibit traversal through" => self.parse_scope(&line, ScopeKind::ProhibitTraversal),
            "exclude from projection" => self.parse_scope(&line, ScopeKind::ExcludeFromProjection),
            _ => self.unresolved(
                line_start,
                content,
                proof::UnresolvedMaterialReason::UnsupportedInterpretation,
            ),
        }
    }

    fn parse_start(&mut self, line: &Line<'_>) {
        self.saw_start_line = true;
        if self.start.is_some() {
            self.unresolved(
                line.start,
                line.content,
                proof::UnresolvedMaterialReason::AmbiguousSelectorResolution,
            );
            return;
        }
        let Some(selector) = sole_token(line.value) else {
            self.unresolved(
                line.start,
                line.content,
                proof::UnresolvedMaterialReason::UnsupportedInterpretation,
            );
            return;
        };
        let Some(parsed) = parse_selector(selector) else {
            self.unresolved(
                line.start,
                line.content,
                proof::UnresolvedMaterialReason::MissingSelectorResolution,
            );
            return;
        };
        self.resolved(line, &[proof::ProofContractField::Start]);
        self.start = Some(parsed.into_symbol());
    }

    fn parse_step(&mut self, line: &Line<'_>) {
        self.saw_step_line = true;
        let line_start = line.start;
        let content = line.content;
        let value_text = line.value;
        let declared = line
            .normalized_label
            .strip_prefix("step ")
            .and_then(|number| number.trim().parse::<usize>().ok());
        // Steps are ordered, so a number that is not the next one leaves the
        // ordering claim unsupported rather than silently renumbering it.
        if declared != Some(self.steps.len() + 1) {
            self.unresolved(
                line_start,
                content,
                proof::UnresolvedMaterialReason::UnsupportedInterpretation,
            );
            return;
        }
        let Ok(step) = u8::try_from(self.steps.len()) else {
            self.unresolved(
                line_start,
                content,
                proof::UnresolvedMaterialReason::UnsupportedInterpretation,
            );
            return;
        };
        let tokens = value_text.split_whitespace().collect::<Vec<_>>();
        let [directness, relation, arrow, target] = tokens[..] else {
            self.unresolved(
                line_start,
                content,
                proof::UnresolvedMaterialReason::UnsupportedInterpretation,
            );
            return;
        };
        if !directness.eq_ignore_ascii_case("direct")
            || !relation.eq_ignore_ascii_case("call")
            || arrow != "->"
        {
            self.unresolved(
                line_start,
                content,
                proof::UnresolvedMaterialReason::UnsupportedInterpretation,
            );
            return;
        }
        let Some(parsed) = parse_selector(target) else {
            self.unresolved(
                line_start,
                content,
                proof::UnresolvedMaterialReason::MissingSelectorResolution,
            );
            return;
        };
        self.resolved(
            line,
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

    fn parse_scope(&mut self, line: &Line<'_>, kind: ScopeKind) {
        let line_start = line.start;
        let content = line.content;
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
        let Some(selector) = sole_token(line.value) else {
            self.unresolved(
                line_start,
                content,
                proof::UnresolvedMaterialReason::UnsupportedInterpretation,
            );
            return;
        };
        let Some(parsed) = parse_selector(selector) else {
            self.unresolved(
                line_start,
                content,
                proof::UnresolvedMaterialReason::MissingSelectorResolution,
            );
            return;
        };
        let field = match kind {
            ScopeKind::ProhibitTraversal => {
                proof::ProofContractField::TraversalProhibition { index }
            }
            ScopeKind::ExcludeFromProjection => {
                proof::ProofContractField::ProjectionExclusion { index }
            }
        };
        self.resolved(line, &[field]);
        match kind {
            ScopeKind::ProhibitTraversal => self
                .prohibit_traversal_through
                .push(parsed.into_scope()),
            ScopeKind::ExcludeFromProjection => {
                self.exclude_from_projection.push(parsed.into_scope())
            }
        }
    }

    /// One line is one clause. The whole line establishes the fields it
    /// declares, so the anchor quotes the caller's own text back rather than
    /// splitting it into tokens the reader never wrote separately.
    fn resolved(&mut self, line: &Line<'_>, fields: &[proof::ProofContractField]) {
        self.push(
            line.start,
            line.content,
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

/// A qualified name, optionally scoped to one project file.
struct ParsedSelector {
    qualified_name: String,
    project_file_components: Option<Vec<String>>,
}

impl ParsedSelector {
    fn into_symbol(self) -> proof::UnvalidatedExactSymbolSelector {
        proof::UnvalidatedExactSymbolSelector::QualifiedName {
            qualified_name: self.qualified_name,
            project_file_components: self.project_file_components,
        }
    }

    fn into_scope(self) -> proof::UnvalidatedExactScopeSelector {
        proof::UnvalidatedExactScopeSelector::QualifiedName {
            qualified_name: self.qualified_name,
            project_file_components: self.project_file_components,
        }
    }
}

/// `crate::module::Alpha` or `"src/gamma.rs"::Gamma`. Nothing else: a signature,
/// a pattern, or a pinned internal node is not a public selector.
fn parse_selector(token: &str) -> Option<ParsedSelector> {
    let (qualified_name, project_file_components) = match token.strip_prefix('"') {
        Some(rest) => {
            let (path, name) = rest.split_once("\"::")?;
            let components = path.split('/').map(str::to_owned).collect::<Vec<_>>();
            if components.iter().any(|component| {
                component.is_empty()
                    || component == "."
                    || component == ".."
                    || component.contains('\\')
                    || component.contains(':')
                    || component.starts_with('~')
            }) {
                return None;
            }
            (name.to_owned(), Some(components))
        }
        None => (token.to_owned(), None),
    };
    if qualified_name.is_empty()
        || qualified_name.chars().any(|character| {
            character.is_whitespace() || matches!(character, '(' | ')' | '*' | '?' | '"' | '\0')
        })
    {
        return None;
    }
    Some(ParsedSelector {
        qualified_name,
        project_file_components,
    })
}

/// One `label: value` line, split at the first colon. Labels never contain one;
/// selectors routinely do.
struct Line<'a> {
    start: usize,
    content: &'a str,
    normalized_label: String,
    value: &'a str,
}

impl<'a> Line<'a> {
    fn split(start: usize, content: &'a str) -> Option<Self> {
        let colon = content.find(':')?;
        let (label, value) = (&content[..colon], &content[colon + 1..]);
        let label = label.trim();
        if label.is_empty() {
            return None;
        }
        Some(Self {
            start,
            content,
            normalized_label: label
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .to_ascii_lowercase(),
            value: value.trim(),
        })
    }
}

fn sole_token(text: &str) -> Option<&str> {
    let mut tokens = text.split_whitespace();
    tokens.next().filter(|_| tokens.next().is_none())
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOCUMENT: &str = concat!(
        "call-path/v1\n",
        "start: crate::module::Alpha\n",
        "step 1: direct call -> crate::module::Beta\n",
        "step 2: direct call -> \"src/gamma.rs\"::Gamma\n",
        "prohibit traversal through: crate::detail::Helper\n",
        "exclude from projection: crate::test_support\n",
    );

    fn validated(document: &str) -> proof::ValidationOutcome {
        let contract = parse_call_path_document(document).expect("parse");
        proof::validate_contract(contract).expect("validate")
    }

    #[test]
    fn the_documented_grammar_validates_with_no_translation_gaps() {
        match validated(DOCUMENT) {
            proof::ValidationOutcome::Validated { .. } => {}
            proof::ValidationOutcome::Unknown { gaps, .. } => {
                panic!("the documented example must validate cleanly, found gaps {gaps:?}")
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
    fn a_missing_or_wrong_version_line_is_a_syntax_error() {
        assert!(parse_call_path_document("").is_err());
        assert!(parse_call_path_document("start: A\nstep 1: direct call -> B\n").is_err());
        assert!(parse_call_path_document("call-path/v2\nstart: A\n").is_err());
    }

    #[test]
    fn a_document_without_a_start_or_a_step_is_a_syntax_error() {
        assert!(
            parse_call_path_document("call-path/v1\nstep 1: direct call -> B\n").is_err(),
            "a contract with no start cannot be built"
        );
        assert!(
            parse_call_path_document("call-path/v1\nstart: A\n").is_err(),
            "a contract with no step cannot be built"
        );
    }

    #[test]
    fn an_uninterpretable_line_becomes_unresolved_material_not_a_silent_skip() {
        let document = concat!(
            "call-path/v1\n",
            "start: crate::module::Alpha\n",
            "step 1: direct call -> crate::module::Beta\n",
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
    fn a_selector_the_grammar_does_not_accept_is_unresolved() {
        for value in [
            "crate::module::Alpha(usize)",
            "crate::module::*",
            "crate::module::Alpha?",
        ] {
            let document =
                format!("call-path/v1\nstart: {value}\nstep 1: direct call -> crate::B\n");
            let error = parse_call_path_document(&document)
                .expect_err("a start that cannot resolve leaves no contract");
            assert!(error.message.contains("start"), "{error}");
        }
    }

    #[test]
    fn an_unresolvable_step_target_reports_unknown_rather_than_proving_fewer_steps() {
        let document = concat!(
            "call-path/v1\n",
            "start: crate::module::Alpha\n",
            "step 1: direct call -> crate::module::Beta\n",
            "step 2: direct call -> crate::module::Gamma(usize)\n",
        );
        let contract = parse_call_path_document(document).expect("parse");
        assert_eq!(contract.spec().steps.len(), 1);
        match proof::validate_contract(contract).expect("validate") {
            proof::ValidationOutcome::Unknown { gaps, .. } => assert!(!gaps.is_empty()),
            proof::ValidationOutcome::Validated { .. } => {
                panic!("a dropped step must not validate as a complete translation")
            }
        }
    }

    #[test]
    fn out_of_order_step_numbers_do_not_renumber_the_contract() {
        let document = concat!(
            "call-path/v1\n",
            "start: crate::module::Alpha\n",
            "step 2: direct call -> crate::module::Beta\n",
            "step 1: direct call -> crate::module::Gamma\n",
        );
        let contract = parse_call_path_document(document).expect("parse");
        // `step 2` arrives first and cannot be step one, so it is unresolved;
        // `step 1` then legitimately becomes the only step.
        assert_eq!(contract.spec().steps.len(), 1);
        match proof::validate_contract(contract).expect("validate") {
            proof::ValidationOutcome::Unknown { .. } => {}
            proof::ValidationOutcome::Validated { .. } => {
                panic!("a misnumbered step must not validate as a complete translation")
            }
        }
    }

    #[test]
    fn a_step_line_missing_its_relation_words_is_unresolved() {
        for value in [
            "step 1: crate::module::Beta",
            "step 1: call -> crate::module::Beta",
            "step 1: direct -> crate::module::Beta",
            "step 1: direct call crate::module::Beta",
            "step 1: direct call => crate::module::Beta",
            "step 1: direct call -> crate::module::Beta extra",
        ] {
            let document = format!("call-path/v1\nstart: crate::A\n{value}\n");
            assert!(
                parse_call_path_document(&document).is_err(),
                "`{value}` must not yield a step"
            );
        }
    }

    #[test]
    fn a_second_start_line_is_ambiguous_rather_than_overwriting_the_first() {
        let document = concat!(
            "call-path/v1\n",
            "start: crate::module::Alpha\n",
            "start: crate::module::Other\n",
            "step 1: direct call -> crate::module::Beta\n",
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
                panic!("a second start must not validate as a complete translation")
            }
        }
    }

    #[test]
    fn blank_lines_and_indentation_are_accepted() {
        let document = concat!(
            "call-path/v1\n",
            "\n",
            "  start: crate::module::Alpha\n",
            "\n",
            "\tstep 1: direct call -> crate::module::Beta\n",
        );
        match validated(document) {
            proof::ValidationOutcome::Validated { .. } => {}
            proof::ValidationOutcome::Unknown { gaps, .. } => panic!("unexpected gaps {gaps:?}"),
        }
    }

    #[test]
    fn scope_lines_are_indexed_in_document_order() {
        let contract = parse_call_path_document(DOCUMENT).expect("parse");
        assert_eq!(contract.spec().prohibit_traversal_through.len(), 1);
        assert_eq!(contract.spec().exclude_from_projection.len(), 1);
    }

    #[test]
    fn a_document_without_a_trailing_newline_still_parses() {
        let document = "call-path/v1\nstart: crate::A\nstep 1: direct call -> crate::B";
        match validated(document) {
            proof::ValidationOutcome::Validated { .. } => {}
            proof::ValidationOutcome::Unknown { gaps, .. } => panic!("unexpected gaps {gaps:?}"),
        }
    }

    #[test]
    fn crlf_line_endings_parse_the_same_way() {
        let document = "call-path/v1\r\nstart: crate::A\r\nstep 1: direct call -> crate::B\r\n";
        match validated(document) {
            proof::ValidationOutcome::Validated { .. } => {}
            proof::ValidationOutcome::Unknown { gaps, .. } => panic!("unexpected gaps {gaps:?}"),
        }
    }
}
