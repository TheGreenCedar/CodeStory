use super::*;

pub(super) fn write_context_bundle<T: serde::Serialize>(
    bundle_dir: &std::path::Path,
    output: &T,
    graphs: &[GraphArtifactDto],
    markdown: &str,
) -> Result<()> {
    fs::create_dir_all(bundle_dir).with_context(|| {
        format!(
            "Failed to create bundle directory {}",
            display::clean_path_string(&bundle_dir.to_string_lossy())
        )
    })?;
    remove_stale_mermaid_artifacts(bundle_dir)?;
    let mut notes = Vec::new();
    let mut omitted_mermaid_artifacts = 0usize;
    let full_context_json =
        serde_json::to_string_pretty(output).context("Failed to serialize context JSON")?;
    let mut context_json = if markdown.len().saturating_add(full_context_json.len())
        > CONTEXT_BUNDLE_OUTPUT_BYTE_CAP
    {
        notes.push(
            "context.json was reduced to a valid manifest summary because the full context exceeded the bundle byte cap."
                .to_string(),
        );
        context_bundle_summary_json(output)?
    } else {
        full_context_json
    };
    if context_json.len() > CONTEXT_BUNDLE_OUTPUT_BYTE_CAP {
        notes.push(
            "context.json details were omitted because the summary still exceeded the bundle byte cap."
                .to_string(),
        );
        let metadata = serde_json::to_value(output)
            .ok()
            .and_then(|value| value.get("_meta").cloned())
            .unwrap_or(serde_json::Value::Null);
        context_json = serde_json::to_string_pretty(&serde_json::json!({
            "truncated": true,
            "reason": "context bundle output hit its byte cap",
            "action": "Narrow the target or use JSON output without --bundle for the full in-memory response.",
            "_meta": metadata,
        }))
        .context("Failed to serialize minimal context bundle summary JSON")?;
    }

    let mut markdown = if markdown.len() > CONTEXT_BUNDLE_MARKDOWN_SOFT_CAP {
        notes.push(format!(
            "context.md was truncated to {} bytes before writing.",
            CONTEXT_BUNDLE_MARKDOWN_SOFT_CAP
        ));
        truncate_utf8_with_suffix(
            markdown,
            CONTEXT_BUNDLE_MARKDOWN_SOFT_CAP,
            CONTEXT_BUNDLE_TRUNCATION_SUFFIX,
        )
    } else {
        markdown.to_string()
    };
    let remaining_markdown_bytes =
        CONTEXT_BUNDLE_OUTPUT_BYTE_CAP.saturating_sub(context_json.len());
    if markdown.len() > remaining_markdown_bytes {
        notes.push(format!(
            "context.md was truncated to fit the remaining {} bundle bytes.",
            remaining_markdown_bytes
        ));
        markdown = truncate_utf8_with_suffix(
            &markdown,
            remaining_markdown_bytes,
            CONTEXT_BUNDLE_TRUNCATION_SUFFIX,
        );
    }
    fs::write(bundle_dir.join("context.md"), &markdown).with_context(|| {
        format!(
            "Failed to write {}",
            display::clean_path_string(&bundle_dir.join("context.md").to_string_lossy())
        )
    })?;
    fs::write(bundle_dir.join("context.json"), &context_json).with_context(|| {
        format!(
            "Failed to write {}",
            display::clean_path_string(&bundle_dir.join("context.json").to_string_lossy())
        )
    })?;
    let mut written_bytes = markdown.len().saturating_add(context_json.len());
    for graph in graphs {
        if let GraphArtifactDto::Mermaid {
            id, mermaid_syntax, ..
        } = graph
        {
            let file_name = format!("{}.mmd", sanitize_artifact_name(id));
            let artifact_path = bundle_dir.join(&file_name);
            if written_bytes.saturating_add(mermaid_syntax.len()) > CONTEXT_BUNDLE_OUTPUT_BYTE_CAP {
                omitted_mermaid_artifacts = omitted_mermaid_artifacts.saturating_add(1);
                continue;
            }
            fs::write(&artifact_path, mermaid_syntax)?;
            written_bytes = written_bytes.saturating_add(mermaid_syntax.len());
        }
    }
    if omitted_mermaid_artifacts > 0 {
        notes.push(format!(
            "Omitted {omitted_mermaid_artifacts} Mermaid artifact(s) after reaching the bundle byte cap."
        ));
    }
    let manifest = serde_json::json!({
        "output_byte_cap": CONTEXT_BUNDLE_OUTPUT_BYTE_CAP,
        "written_bytes_excluding_manifest": written_bytes,
        "truncated": !notes.is_empty(),
        "omitted_mermaid_artifacts": omitted_mermaid_artifacts,
        "notes": notes,
    });
    fs::write(
        bundle_dir.join("bundle_manifest.json"),
        serde_json::to_string_pretty(&manifest).context("Failed to serialize bundle manifest")?,
    )
    .with_context(|| {
        format!(
            "Failed to write {}",
            display::clean_path_string(&bundle_dir.join("bundle_manifest.json").to_string_lossy())
        )
    })?;
    Ok(())
}

pub(super) fn remove_stale_mermaid_artifacts(bundle_dir: &std::path::Path) -> Result<()> {
    for entry in fs::read_dir(bundle_dir).with_context(|| {
        format!(
            "Failed to inspect bundle directory {}",
            display::clean_path_string(&bundle_dir.to_string_lossy())
        )
    })? {
        let entry = entry.context("Failed to inspect bundle entry")?;
        let path = entry.path();
        if path.extension().is_some_and(|extension| extension == "mmd") {
            fs::remove_file(&path).with_context(|| {
                format!(
                    "Failed to remove stale {}",
                    display::clean_path_string(&path.to_string_lossy())
                )
            })?;
        }
    }
    Ok(())
}

pub(super) fn context_bundle_summary_json<T: serde::Serialize>(output: &T) -> Result<String> {
    let value = serde_json::to_value(output).context("Failed to serialize context summary JSON")?;
    serde_json::to_string_pretty(&serde_json::json!({
        "truncated": true,
        "reason": "context bundle output hit its byte cap",
        "action": "Narrow the target or use JSON output without --bundle for the full in-memory response.",
        "_meta": value.get("_meta"),
        "target": value.get("target"),
        "resolution": value.get("resolution"),
        "context_summary": value.pointer("/context/summary"),
        "citation_count": value
            .pointer("/context/citations")
            .and_then(|citations| citations.as_array())
            .map(|citations| citations.len())
            .unwrap_or(0),
        "graph_count": value
            .pointer("/context/graphs")
            .and_then(|graphs| graphs.as_array())
            .map(|graphs| graphs.len())
            .unwrap_or(0),
    }))
    .context("Failed to serialize context bundle summary JSON")
}

pub(super) fn truncate_utf8_with_suffix(value: &str, max_bytes: usize, suffix: &str) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let mut keep = max_bytes.saturating_sub(suffix.len());
    while keep > 0 && !value.is_char_boundary(keep) {
        keep -= 1;
    }
    let mut truncated = value[..keep].to_string();
    truncated.push_str(suffix);
    if truncated.len() > max_bytes {
        let mut hard_keep = max_bytes;
        while hard_keep > 0 && !truncated.is_char_boundary(hard_keep) {
            hard_keep -= 1;
        }
        truncated.truncate(hard_keep);
    }
    truncated
}

pub(super) fn sanitize_artifact_name(value: &str) -> String {
    let mut out = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>();
    if out.is_empty() {
        out.push_str("artifact");
    }
    out
}

pub(crate) fn ensure_dot_only_for_trail(format: args::OutputFormat, command: &str) -> Result<()> {
    if format == args::OutputFormat::Dot {
        bail!("--format dot is only supported by `trail`; `{command}` supports markdown and json");
    }
    Ok(())
}
