use crate::agent::eval_probes::eval_probes_enabled;
use crate::agent::packet_citations::{
    packet_citation_matching_display, packet_citation_matching_path_and_display,
    packet_command_crate_sources_contain_all,
};
use crate::agent::packet_scoring::normalize_identifier;
use codestory_contracts::api::{AgentCitationDto, PacketClaimDto, PacketTaskClassDto};
use std::collections::HashSet;

#[derive(Debug, Clone)]
struct PacketCommandDescriptor {
    command_title: String,
    subcommand_title: String,
    module: String,
    crate_segment: String,
}

fn packet_command_descriptors(question: &str) -> Vec<PacketCommandDescriptor> {
    let mut descriptors = Vec::new();
    for span in packet_backtick_spans(question) {
        let words = packet_command_words(span);
        if words.len() < 2 {
            continue;
        }
        let command = &words[0];
        let subcommand = &words[1];
        let Some(command_title) = packet_pascal_identifier(command) else {
            continue;
        };
        let Some(subcommand_title) = packet_pascal_identifier(subcommand) else {
            continue;
        };
        let Some(module) = packet_snake_identifier(&[command.as_str(), subcommand.as_str()]) else {
            continue;
        };
        let Some(crate_segment) = packet_snake_identifier(&[subcommand.as_str()]) else {
            continue;
        };
        descriptors.push(PacketCommandDescriptor {
            command_title,
            subcommand_title,
            module,
            crate_segment,
        });
    }
    descriptors
}

pub(crate) fn packet_command_exact_probe_queries(
    question: &str,
    task_class: PacketTaskClassDto,
) -> Vec<String> {
    if !eval_probes_enabled() || !packet_allows_command_probe_queries(question, task_class) {
        return Vec::new();
    }

    let mut queries = Vec::new();
    for descriptor in packet_command_descriptors(question) {
        push_unique_term(
            &mut queries,
            &format!("Subcommand::{}", descriptor.subcommand_title),
        );
        push_unique_term(&mut queries, &format!("{}::Cli", descriptor.module));
        push_unique_term(&mut queries, &format!("{}::run_main", descriptor.module));
    }
    queries
}

pub(crate) fn packet_command_role_probe_queries(
    question: &str,
    task_class: PacketTaskClassDto,
) -> Vec<String> {
    if !packet_allows_command_probe_queries(question, task_class) {
        return Vec::new();
    }

    let mut queries = Vec::new();
    for descriptor in packet_command_descriptors(question) {
        let command_phrase = descriptor.module.replace('_', " ");
        let subcommand_phrase = descriptor.subcommand_title.to_ascii_lowercase();
        push_unique_term(&mut queries, &command_phrase);
        push_unique_term(&mut queries, &format!("{command_phrase} command"));
        push_unique_term(&mut queries, &format!("{subcommand_phrase} command"));
        push_unique_term(&mut queries, &format!("{subcommand_phrase} subcommand"));
    }
    queries
}

fn packet_allows_command_probe_queries(question: &str, task_class: PacketTaskClassDto) -> bool {
    if !matches!(
        task_class,
        PacketTaskClassDto::ArchitectureExplanation
            | PacketTaskClassDto::DataFlow
            | PacketTaskClassDto::ChangeImpact
            | PacketTaskClassDto::RouteTracing
            | PacketTaskClassDto::EditPlanning
    ) {
        return false;
    }
    let lowered = question.to_ascii_lowercase();
    contains_any(
        &lowered,
        &[
            "cli",
            "command",
            "subcommand",
            "entrypoint",
            "entry point",
            "runtime",
            "flow",
            "flows",
        ],
    )
}

pub(crate) fn packet_append_command_flow_template_claims(
    prompt: &str,
    citations: &[AgentCitationDto],
    claims: &mut Vec<PacketClaimDto>,
    seen: &mut HashSet<String>,
) {
    let normalized_prompt = normalize_identifier(prompt);
    if !(normalized_prompt.contains("cli")
        || normalized_prompt.contains("command")
        || normalized_prompt.contains("subcommand"))
    {
        return;
    }

    for descriptor in packet_command_descriptors(prompt) {
        let subcommand_display = format!("Subcommand::{}", descriptor.subcommand_title);
        let cli_display = format!("{}::Cli", descriptor.module);
        let run_main_display = format!("{}::run_main", descriptor.module);
        let subcommand_citation = packet_citation_matching_display(citations, &subcommand_display);
        let cli_citation = packet_citation_matching_display(citations, &cli_display);
        let run_main_citation = packet_citation_matching_display(citations, &run_main_display)
            .or_else(|| {
                packet_citation_matching_path_and_display(
                    citations,
                    &descriptor.crate_segment,
                    "run_main",
                )
            });

        if let Some(subcommand_citation) = subcommand_citation
            && (cli_citation.is_some() || run_main_citation.is_some())
        {
            let mut claim_citations = vec![subcommand_citation.clone()];
            if let Some(cli_citation) = cli_citation {
                claim_citations.push(cli_citation.clone());
            } else if let Some(run_main_citation) = run_main_citation {
                claim_citations.push(run_main_citation.clone());
            }
            let claim = format!(
                "The top-level {} CLI has a cited {} subcommand and command-module entrypoint in `{}`.",
                descriptor.command_title, descriptor.subcommand_title, descriptor.module
            );
            packet_push_flow_template_claim_with_citations(claims, seen, &claim, claim_citations);
        }

        if let Some(cli_citation) = cli_citation
            && let Some(run_main_citation) = run_main_citation
        {
            packet_push_flow_template_claim_with_citations(
                claims,
                seen,
                &format!(
                    "The {} binary parses {}-specific CLI options and calls {}::run_main.",
                    descriptor.module.replace('_', "-"),
                    descriptor.crate_segment,
                    descriptor.module
                ),
                vec![cli_citation.clone(), run_main_citation.clone()],
            );
            if (normalized_prompt.contains("json") || normalized_prompt.contains("jsonl"))
                && packet_command_crate_sources_contain_all(
                    citations,
                    &descriptor.crate_segment,
                    &[&["long = \"json\"", "--json"], &["jsonl"]],
                )
            {
                packet_push_flow_template_claim(
                    claims,
                    seen,
                    &format!(
                        "The {} CLI defines --json as the switch that chooses JSONL stdout output.",
                        descriptor.crate_segment
                    ),
                    Some(cli_citation.clone()),
                );
            }
        }

        let runtime_citation = run_main_citation.or_else(|| {
            packet_citation_matching_path_and_display(
                citations,
                &descriptor.crate_segment,
                "run_exec_session",
            )
        });
        if let Some(runtime_citation) = runtime_citation
            && (normalized_prompt.contains("appserver")
                || normalized_prompt.contains("runtime")
                || normalized_prompt.contains("thread")
                || normalized_prompt.contains("turn"))
            && packet_command_crate_sources_contain_all(
                citations,
                &descriptor.crate_segment,
                &[
                    &[
                        "configbuilder",
                        "configbuilder::default",
                        "configbuilder::default()",
                    ],
                    &["approval"],
                    &["sandbox"],
                    &["inprocessclientstartargs"],
                ],
            )
        {
            packet_push_flow_template_claim(
                claims,
                seen,
                "run_main loads config, resolves sandbox and approval settings, and builds the in-process app-server start arguments.",
                Some(runtime_citation.clone()),
            );
        }
    }
}

fn packet_backtick_spans(question: &str) -> Vec<&str> {
    let mut spans = Vec::new();
    let mut start = None;
    for (index, ch) in question.char_indices() {
        if ch != '`' {
            continue;
        }
        if let Some(open) = start.take() {
            let span = question[open..index].trim();
            if !span.is_empty() {
                spans.push(span);
            }
        } else {
            start = Some(index + ch.len_utf8());
        }
    }
    spans
}

fn packet_command_words(span: &str) -> Vec<String> {
    span.split_whitespace()
        .filter_map(|token| {
            let token = token.trim_matches(|ch: char| {
                matches!(
                    ch,
                    ',' | '.'
                        | ';'
                        | ':'
                        | '?'
                        | '!'
                        | '('
                        | ')'
                        | '['
                        | ']'
                        | '{'
                        | '}'
                        | '"'
                        | '\''
                )
            });
            if token.starts_with('-')
                || token.is_empty()
                || !token.chars().any(|ch| ch.is_ascii_alphabetic())
                || !token
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
            {
                return None;
            }
            Some(token.to_string())
        })
        .take(3)
        .collect()
}

fn packet_pascal_identifier(word: &str) -> Option<String> {
    let mut value = String::new();
    for part in word
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|part| !part.is_empty())
    {
        let mut chars = part.chars();
        let first = chars.next()?;
        value.push(first.to_ascii_uppercase());
        value.extend(chars.map(|ch| ch.to_ascii_lowercase()));
    }
    (!value.is_empty()).then_some(value)
}

fn packet_snake_identifier(words: &[&str]) -> Option<String> {
    let mut parts = Vec::new();
    for word in words {
        let mut normalized = String::new();
        for (index, part) in word
            .split(|ch: char| !ch.is_ascii_alphanumeric())
            .filter(|part| !part.is_empty())
            .enumerate()
        {
            if index > 0 {
                normalized.push('_');
            }
            normalized.push_str(&part.to_ascii_lowercase());
        }
        if normalized.is_empty() {
            return None;
        }
        parts.push(normalized);
    }
    (!parts.is_empty()).then_some(parts.join("_"))
}

fn packet_push_flow_template_claim(
    claims: &mut Vec<PacketClaimDto>,
    seen: &mut HashSet<String>,
    claim_text: &str,
    citation: Option<AgentCitationDto>,
) {
    packet_push_flow_template_claim_with_citations(
        claims,
        seen,
        claim_text,
        citation.map(|value| vec![value]).unwrap_or_default(),
    );
}

fn packet_push_flow_template_claim_with_citations(
    claims: &mut Vec<PacketClaimDto>,
    seen: &mut HashSet<String>,
    claim_text: &str,
    citations: Vec<AgentCitationDto>,
) {
    let key = normalize_identifier(claim_text);
    if key.is_empty() || !seen.insert(key) {
        return;
    }
    claims.push(PacketClaimDto {
        claim: claim_text.to_string(),
        citations,
    });
}

fn push_unique_term(terms: &mut Vec<String>, value: &str) {
    if value.is_empty() || terms.iter().any(|term| term == value) {
        return;
    }
    terms.push(value.to_string());
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}
