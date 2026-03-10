use codestory_api::{GraphResponse, SearchHit};
use std::collections::HashMap;
use std::fmt::Write as _;

fn sanitize_mermaid_label(input: &str) -> String {
    input.replace('"', "'").replace('\n', " ")
}

pub(crate) fn sanitize_mermaid_text(input: &str) -> String {
    let mut sanitized = input.replace('"', "").replace(['\n', '\r'], " ");
    sanitized = sanitized
        .chars()
        .map(|ch| if ch.is_ascii_control() { ' ' } else { ch })
        .collect::<String>();
    let collapsed = sanitized.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        "request".to_string()
    } else {
        collapsed
    }
}

pub(crate) fn mermaid_flowchart(graph: &GraphResponse) -> String {
    let mut out = String::from("flowchart LR\n");
    for node in graph.nodes.iter().take(14) {
        let _ = writeln!(
            out,
            "    N{}[\"{}\"]",
            node.id.0,
            sanitize_mermaid_label(&node.label)
        );
    }

    for edge in graph.edges.iter().take(20) {
        let _ = writeln!(
            out,
            "    N{} -->|\"{:?}\"| N{}",
            edge.source.0, edge.kind, edge.target.0
        );
    }

    out
}

pub(crate) fn mermaid_sequence(graph: &GraphResponse) -> String {
    let mut out = String::from("sequenceDiagram\n");
    let mut labels: HashMap<String, String> = HashMap::new();
    for node in graph.nodes.iter().take(10) {
        labels.insert(node.id.0.clone(), node.label.clone());
    }

    let mut emitted = 0usize;
    for edge in graph.edges.iter().take(14) {
        let Some(source) = labels.get(&edge.source.0) else {
            continue;
        };
        let Some(target) = labels.get(&edge.target.0) else {
            continue;
        };

        emitted += 1;
        let _ = writeln!(
            out,
            "    {}->>{}: {:?}",
            sanitize_mermaid_label(source),
            sanitize_mermaid_label(target),
            edge.kind
        );
    }

    if emitted == 0 {
        out.push_str("    User->>System: No sequencing data available\n");
    }
    out
}

pub(crate) fn mermaid_gantt(citations: &[SearchHit]) -> String {
    let mut out = String::from("gantt\n    title Investigation Plan\n    dateFormat X\n");
    let mut current = 0u32;
    for (idx, hit) in citations.iter().take(5).enumerate() {
        let duration = 1 + (idx as u32 % 2);
        let _ = writeln!(
            out,
            "    {} :{}, {}, {}",
            sanitize_mermaid_label(&hit.display_name),
            idx + 1,
            current,
            duration
        );
        current += duration;
    }
    if citations.is_empty() {
        out.push_str("    Baseline scan :1, 0, 1\n");
    }
    out
}

pub(crate) fn fallback_mermaid(prompt: &str, hit_count: usize) -> String {
    let prompt_summary = prompt
        .split_whitespace()
        .take(6)
        .collect::<Vec<_>>()
        .join(" ");

    format!(
        "flowchart LR\n    A[\"Prompt\"] --> B[\"{}\"]\n    B --> C[\"Indexed hits: {}\"]\n    C --> D[\"Refine symbol names or run indexing\"]\n",
        sanitize_mermaid_text(&prompt_summary),
        hit_count
    )
}
