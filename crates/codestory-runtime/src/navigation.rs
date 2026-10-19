//! Passive, source-verified CALL leads from untrusted locator data.
use crate::{AppController, node_display_name};
use codestory_contracts::api::ApiError;
use codestory_contracts::graph::{Node, NodeKind, OccurrenceKind, ResolutionCertainty};
use codestory_store::Store;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NavigationRequest {
    pub locations: Vec<NavigationLocation>,
    #[serde(default)]
    pub input_paths: Vec<String>,
    #[serde(default)]
    pub candidate_limit_reached: bool,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NavigationLocation {
    pub path: Option<String>,
    pub line: u32,
    pub text: String,
}

#[derive(Debug, serde::Serialize)]
pub struct NavigationSnapshot {
    pub project_root: String,
    pub generation_id: String,
    pub run_id: String,
    pub candidates_considered: usize,
    pub candidate_limit_reached: bool,
    pub target_limit_reached: bool,
    pub skipped_locations: usize,
    pub rows: Vec<NavigationLead>,
}

#[derive(Debug, serde::Serialize)]
pub struct NavigationLead {
    pub edge_id: String,
    pub call_line: u32,
    pub source: NavigationDefinition,
    pub target: NavigationDefinition,
}

#[derive(Debug, serde::Serialize)]
pub struct NavigationDefinition {
    pub id: String,
    pub name: String,
    pub path: String,
    pub start_line: u32,
    pub end_line: u32,
    pub content_sha256: String,
}

struct VerifiedFile {
    path: String,
    identity: codestory_workspace::WorkspacePathIdentity,
    source: crate::search_evidence::VerifiedSourceFile,
}

fn literal_path(root: &Path, value: &str) -> Option<(PathBuf, PathBuf)> {
    if value.is_empty() || value.len() > 4096 {
        return None;
    }
    let canonical =
        crate::path_resolution::resolve_project_file_path_from_root(root, value, false).ok()?;
    let raw = root.join(value);
    crate::controller_files::open_regular_source_file_with(&canonical, |p| std::fs::File::open(p))
        .ok()?;
    Some((raw, canonical))
}

fn callable(node: &Node) -> bool {
    matches!(node.kind, NodeKind::FUNCTION | NodeKind::METHOD)
}

fn declaration_ranges(
    storage: &Store,
    node: &Node,
) -> Vec<codestory_contracts::graph::SourceLocation> {
    let Ok(occurrences) = storage.get_occurrences_for_element(node.id.0) else {
        return Vec::new();
    };
    let mut ranges = occurrences
        .into_iter()
        .filter(|o| {
            matches!(
                o.kind,
                OccurrenceKind::DEFINITION | OccurrenceKind::DECLARATION
            ) && Some(o.location.file_node_id) == node.file_node_id
                && o.location.start_line > 0
                && o.location.end_line >= o.location.start_line
        })
        .map(|o| o.location)
        .collect::<Vec<_>>();
    ranges.sort_by_key(|r| (r.start_line, r.start_col, r.end_line, r.end_col));
    ranges.dedup();
    ranges
}

impl AppController {
    pub(crate) fn passive_navigation_at(
        &self,
        root: PathBuf,
        storage_path: PathBuf,
        request: &NavigationRequest,
    ) -> Result<NavigationSnapshot, ApiError> {
        if !root.is_absolute() || !storage_path.is_absolute() || request.input_paths.len() > 32 {
            return Err(ApiError::invalid_argument(
                "Invalid passive navigation scope",
            ));
        }
        let canonical_root = root
            .canonicalize()
            .map_err(|e| ApiError::invalid_argument(e.to_string()))?;
        self.bind_project_paths_for_refresh(root.clone(), storage_path)?;
        self.with_complete_core_snapshot(|publication| {
            let storage = self.open_storage_read_only()?;
            let mut files: HashMap<i64, Option<Rc<VerifiedFile>>> = HashMap::new();
            let mut verify = |id: i64| -> Option<Rc<VerifiedFile>> {
                files
                    .entry(id)
                    .or_insert_with(|| {
                        let file = storage.get_file_by_id(id).ok()??;
                        if !file.indexed || !file.complete {
                            return None;
                        }
                        let absolute = crate::path_resolution::resolve_project_file_path_from_root(
                            &root,
                            file.path.to_str()?,
                            false,
                        )
                        .ok()?;
                        let identity =
                            codestory_workspace::workspace_path_identity(&absolute).ok()?;
                        let path = absolute
                            .strip_prefix(&canonical_root)
                            .ok()?
                            .to_str()?
                            .replace(std::path::MAIN_SEPARATOR, "/");
                        let cap = self.source_index_policy.effective_byte_cap(&path);
                        let source = crate::search_evidence::verified_file_bounded(
                            &storage, &root, &file, cap,
                        )
                        .ok()?;
                        if codestory_workspace::workspace_path_identity(&source.path).ok()?
                            != identity
                        {
                            return None;
                        }

                        Some(Rc::new(VerifiedFile {
                            path,
                            identity,
                            source,
                        }))
                    })
                    .clone()
            };
            // Literal command tokens are only candidates. Count all contained regular
            // files, including unindexed ones, before resolving a pathless result.
            let mut literals = Vec::new();
            let mut identities = Vec::new();
            for token in &request.input_paths {
                if let Some((raw, canonical)) = literal_path(&root, token) {
                    if let Ok(identity) = codestory_workspace::workspace_path_identity(&canonical) {
                        if !identities.contains(&identity) {
                            identities.push(identity);
                            literals.push((raw, canonical));
                        }
                    }
                }
            }
            let mut seeds = Vec::new();
            let mut known = HashSet::new();
            let mut skipped = 0;
            for location in request.locations.iter().take(32) {
                let paths = match &location.path {
                    Some(path) => literal_path(&root, path),
                    None if literals.len() == 1 => Some(literals[0].clone()),
                    None => None,
                };
                let resolved = (|| {
                    let (raw, canonical) = paths?;
                    let file = storage
                        .get_file_by_path(&raw)
                        .ok()?
                        .or_else(|| storage.get_file_by_path(&canonical).ok().flatten())
                        // Rehydrate a contained native alias under the explicit
                        // root spelling used by the indexed publication.
                        .or_else(|| {
                            canonical
                                .strip_prefix(&canonical_root)
                                .ok()
                                .and_then(|relative| {
                                    storage
                                        .get_file_by_path(&root.join(relative))
                                        .ok()
                                        .flatten()
                                })
                        })?;
                    let verified = verify(file.id)?;
                    if location.line == 0
                        || location.text.len() > 4096
                        || codestory_workspace::workspace_path_identity(&canonical).ok()?
                            != verified.identity
                        || verified
                            .source
                            .content
                            .lines()
                            .nth(location.line as usize - 1)?
                            != location.text
                    {
                        return None;
                    }
                    let mut candidates = storage
                        .get_nodes_for_file_line(file.path.to_str()?, location.line)
                        .ok()?
                        .into_iter()
                        .filter(callable)
                        .flat_map(|n| {
                            declaration_ranges(&storage, &n)
                                .into_iter()
                                .filter(|r| {
                                    r.start_line <= location.line && location.line <= r.end_line
                                })
                                .map(move |r| (n.clone(), r))
                        })
                        .collect::<Vec<_>>();
                    candidates.sort_by_key(|(n, r)| {
                        (
                            r.end_line - r.start_line,
                            r.end_col.saturating_sub(r.start_col),
                            n.id.0,
                        )
                    });
                    candidates.dedup_by_key(|(n, _)| n.id);
                    let (node, range) = candidates.first()?;
                    if candidates.get(1).is_some_and(|(_, r)| {
                        (
                            r.end_line - r.start_line,
                            r.end_col.saturating_sub(r.start_col),
                        ) == (
                            range.end_line - range.start_line,
                            range.end_col.saturating_sub(range.start_col),
                        )
                    }) {
                        return None;
                    }
                    Some((node.clone(), range.clone(), verified))
                })();
                if let Some((node, range, file)) = resolved {
                    if known.insert(node.id) {
                        seeds.push((node, range, file));
                    }
                } else {
                    skipped += 1;
                }
            }
            let mut rows = Vec::new();
            let mut emitted = HashSet::new();
            for (node, range, source) in seeds {
                let mut edges = storage
                    .get_edges_for_node_id(node.id)
                    .map_err(|e| ApiError::internal(e.to_string()))?;
                edges.sort_by_key(|e| (e.line.unwrap_or(u32::MAX), e.id.0));
                for edge in edges {
                    let Some(line) = edge.line else { continue };
                    if edge.kind != codestory_contracts::graph::EdgeKind::CALL
                        || edge.effective_source() != node.id
                        || edge.file_node_id != node.file_node_id
                        || line < range.start_line
                        || line > range.end_line
                        || edge.certainty != Some(ResolutionCertainty::Certain)
                    {
                        continue;
                    }
                    let Some(target_id) = edge.resolved_target else {
                        continue;
                    };
                    if known.contains(&target_id) || emitted.contains(&target_id) {
                        continue;
                    }
                    let Some(target) = storage
                        .get_node(target_id)
                        .map_err(|e| ApiError::internal(e.to_string()))?
                        .filter(callable)
                    else {
                        continue;
                    };
                    let ranges = declaration_ranges(&storage, &target);
                    if ranges.len() != 1 {
                        continue;
                    }
                    let target_range = &ranges[0];
                    let Some(target_source) = verify(target_range.file_node_id.0) else {
                        continue;
                    };
                    if target_source
                        .source
                        .content
                        .lines()
                        .nth(target_range.end_line as usize - 1)
                        .is_none()
                    {
                        continue;
                    }
                    let definition =
                        |n: &Node,
                         r: &codestory_contracts::graph::SourceLocation,
                         f: &VerifiedFile| NavigationDefinition {
                            id: n.id.0.to_string(),
                            name: node_display_name(n),
                            path: f.path.clone(),
                            start_line: r.start_line,
                            end_line: r.end_line,
                            content_sha256: f.source.content_sha256.clone(),
                        };
                    rows.push(NavigationLead {
                        edge_id: edge.id.0.to_string(),
                        call_line: line,
                        source: definition(&node, &range, &source),
                        target: definition(&target, target_range, &target_source),
                    });
                    emitted.insert(target_id);
                    if rows.len() == 3 {
                        break;
                    }
                }
                if rows.len() == 3 {
                    break;
                }
            }
            Ok(NavigationSnapshot {
                project_root: root.to_string_lossy().into_owned(),
                generation_id: publication.generation_id.clone(),
                run_id: publication.run_id.clone(),
                candidates_considered: request.locations.len().min(32),
                candidate_limit_reached: request.candidate_limit_reached
                    || request.locations.len() > 32,
                target_limit_reached: rows.len() == 3,
                skipped_locations: skipped,
                rows,
            })
        })
    }
}
