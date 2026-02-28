import { useMemo, useState, type ChangeEvent, type ReactNode } from "react";

import type { LeftTab } from "../app/types";
import { MermaidDiagram } from "./MermaidDiagram";
import { AdvancedSettingsDrawer, type ResolvedCustomConfig } from "./AdvancedSettingsDrawer";
import type {
  AgentAnswerDto,
  AgentCitationDto,
  AgentConnectionSettingsDto,
  AgentResponseBlockDto,
  AgentRetrievalProfileSelectionDto,
  AgentRetrievalPresetDto,
  GraphArtifactDto,
  SymbolSummaryDto,
} from "../generated/api";

type ResponsePaneProps = {
  selectedTab: LeftTab;
  onSelectTab: (tab: LeftTab) => void;
  prompt: string;
  onPromptChange: (prompt: string) => void;
  retrievalProfile: AgentRetrievalProfileSelectionDto;
  onRetrievalProfileChange: (next: AgentRetrievalProfileSelectionDto) => void;
  agentBackend: NonNullable<AgentConnectionSettingsDto["backend"]>;
  onAgentBackendChange: (backend: NonNullable<AgentConnectionSettingsDto["backend"]>) => void;
  agentCommand: string;
  onAgentCommandChange: (command: string) => void;
  onAskAgent: () => void;
  isBusy: boolean;
  projectOpen: boolean;
  agentAnswer: AgentAnswerDto | null;
  graphMap: Record<string, GraphArtifactDto>;
  onActivateGraph: (graphId: string) => void;
  rootSymbols: SymbolSummaryDto[];
  childrenByNode: Record<string, SymbolSummaryDto[]>;
  expandedNodes: Record<string, boolean>;
  onToggleNode: (node: SymbolSummaryDto) => Promise<void>;
  onFocusSymbol: (symbolId: string, label: string) => void;
  activeSymbolId: string | null;
};

type ExplorerEntry = {
  node: SymbolSummaryDto;
  displayLabel: string;
  duplicateCount: number;
  isDependency: boolean;
};

const WRAPPING_QUOTES = new Set(["'", '"', "`", "“", "”", "‘", "’"]);

const DEFAULT_CUSTOM_PROFILE_CONFIG: ResolvedCustomConfig = {
  depth: 3,
  direction: "Both",
  edge_filter: [],
  node_filter: [],
  max_nodes: 800,
  include_edge_occurrences: false,
  enable_source_reads: true,
};

const PRESET_LABELS: Record<AgentRetrievalPresetDto, string> = {
  architecture: "Architecture",
  callflow: "Call Flow",
  inheritance: "Inheritance",
  impact: "Impact",
};

function normalizeSymbolLabel(label: string): string {
  const trimmed = label.trim();
  if (trimmed.length >= 2) {
    const first = trimmed[0] ?? "";
    const last = trimmed[trimmed.length - 1] ?? "";
    if (WRAPPING_QUOTES.has(first) && WRAPPING_QUOTES.has(last)) {
      return trimmed.slice(1, -1);
    }
  }
  return trimmed;
}

function isLikelyDependencySymbol(node: SymbolSummaryDto, displayLabel: string): boolean {
  const normalizedPath = node.file_path?.replace(/\\/g, "/");
  if (normalizedPath?.includes("/node_modules/")) {
    return true;
  }

  if (normalizedPath) {
    return false;
  }

  if (node.kind === "BUILTIN_TYPE") {
    return true;
  }

  if (node.kind === "MODULE" || node.kind === "PACKAGE") {
    const label = displayLabel.toLowerCase();
    return !(label.startsWith("./") || label.startsWith("../") || label.startsWith("/"));
  }

  return false;
}

function buildExplorerEntries(
  nodes: SymbolSummaryDto[],
  collapseDuplicates: boolean,
): ExplorerEntry[] {
  if (!collapseDuplicates) {
    return nodes.map((node) => {
      const displayLabel = normalizeSymbolLabel(node.label);
      return {
        node,
        displayLabel,
        duplicateCount: 1,
        isDependency: isLikelyDependencySymbol(node, displayLabel),
      };
    });
  }

  const grouped = new Map<string, ExplorerEntry>();
  for (const node of nodes) {
    const displayLabel = normalizeSymbolLabel(node.label);
    const key = `${node.kind}\u0000${displayLabel}\u0000${node.file_path ?? ""}`;
    const existing = grouped.get(key);
    if (existing) {
      existing.duplicateCount += 1;
      if (!existing.node.has_children && node.has_children) {
        existing.node = {
          ...existing.node,
          has_children: true,
        };
      }
      continue;
    }

    grouped.set(key, {
      node,
      displayLabel,
      duplicateCount: 1,
      isDependency: isLikelyDependencySymbol(node, displayLabel),
    });
  }

  return [...grouped.values()];
}

function asCustomConfig(profile: AgentRetrievalProfileSelectionDto): ResolvedCustomConfig {
  if (profile.kind === "custom") {
    return {
      depth:
        typeof profile.config.depth === "number"
          ? Math.max(0, Math.trunc(profile.config.depth))
          : 3,
      direction:
        profile.config.direction === "Incoming" || profile.config.direction === "Outgoing"
          ? profile.config.direction
          : "Both",
      edge_filter: Array.isArray(profile.config.edge_filter) ? profile.config.edge_filter : [],
      node_filter: Array.isArray(profile.config.node_filter) ? profile.config.node_filter : [],
      max_nodes:
        typeof profile.config.max_nodes === "number"
          ? Math.max(10, Math.trunc(profile.config.max_nodes))
          : 800,
      include_edge_occurrences: Boolean(profile.config.include_edge_occurrences),
      enable_source_reads:
        typeof profile.config.enable_source_reads === "boolean"
          ? profile.config.enable_source_reads
          : true,
    };
  }
  return DEFAULT_CUSTOM_PROFILE_CONFIG;
}

function sectionGraphs(
  section: AgentAnswerDto["sections"][number],
  graphMap: Record<string, GraphArtifactDto>,
): GraphArtifactDto[] {
  const uniqueGraphIds = new Set(
    section.blocks.filter((block) => block.kind === "mermaid").map((block) => block.graph_id),
  );
  return [...uniqueGraphIds]
    .map((graphId) => graphMap[graphId])
    .filter((graph): graph is GraphArtifactDto => Boolean(graph));
}

function citationLocationLabel(citation: AgentCitationDto): string {
  if (!citation.file_path) {
    return "Unknown location";
  }
  if (citation.line === null) {
    return citation.file_path;
  }
  return `${citation.file_path}:${citation.line}`;
}

function responseBlockToMarkdown(
  block: AgentResponseBlockDto,
  graphMap: Record<string, GraphArtifactDto>,
): string {
  if (block.kind === "markdown") {
    return block.markdown;
  }

  const graph = graphMap[block.graph_id];
  if (!graph) {
    return `Mermaid graph \`${block.graph_id}\` is unavailable in this payload.`;
  }

  if (graph.kind !== "mermaid") {
    return `Graph \`${graph.title}\` is available in the graph pane.`;
  }

  return `\`\`\`mermaid\n${graph.mermaid_syntax}\n\`\`\``;
}

function sanitizeFileName(baseName: string): string {
  const normalized = baseName
    .trim()
    .toLowerCase()
    .replace(/\s+/g, "-")
    .replace(/[^a-z0-9_.-]/g, "");
  return normalized.length > 0 ? normalized : "agent-summary";
}

function buildMarkdownSummary(
  answer: AgentAnswerDto,
  graphMap: Record<string, GraphArtifactDto>,
): string {
  const lines: string[] = [];
  lines.push("# Codestory Answer Summary");
  lines.push("");
  lines.push(`- Prompt: ${answer.prompt}`);
  lines.push(`- Answer ID: ${answer.answer_id}`);
  lines.push("");
  lines.push("## Summary");
  lines.push(answer.summary);
  lines.push("");
  lines.push("## Evidence");
  if (answer.citations.length === 0) {
    lines.push("- No citations were returned.");
  } else {
    for (const citation of answer.citations) {
      lines.push(
        `- ${citation.display_name} (${citation.kind}) - ${citationLocationLabel(citation)} - score ${citation.score.toFixed(3)}`,
      );
    }
  }
  lines.push("");
  lines.push("## Sections");
  for (const section of answer.sections) {
    lines.push(`### ${section.title}`);
    lines.push("");
    for (const block of section.blocks) {
      lines.push(responseBlockToMarkdown(block, graphMap));
      lines.push("");
    }
  }
  return lines.join("\n").trimEnd();
}

function buildJsonSummary(
  answer: AgentAnswerDto,
  graphMap: Record<string, GraphArtifactDto>,
): string {
  return JSON.stringify(
    {
      answer_id: answer.answer_id,
      prompt: answer.prompt,
      summary: answer.summary,
      sections: answer.sections.map((section) => ({
        id: section.id,
        title: section.title,
        blocks: section.blocks.map((block) => {
          if (block.kind === "markdown") {
            return { kind: "markdown", markdown: block.markdown };
          }
          const graph = graphMap[block.graph_id];
          return {
            kind: "mermaid",
            graph_id: block.graph_id,
            graph_title: graph?.title ?? null,
            mermaid_syntax: graph?.kind === "mermaid" ? graph.mermaid_syntax : null,
          };
        }),
      })),
      citations: answer.citations.map((citation) => ({
        node_id: citation.node_id,
        display_name: citation.display_name,
        kind: citation.kind,
        file_path: citation.file_path,
        line: citation.line,
        score: citation.score,
      })),
    },
    null,
    2,
  );
}

function triggerTextDownload(fileName: string, content: string, mimeType: string): void {
  const blob = new Blob([content], { type: `${mimeType};charset=utf-8` });
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = fileName;
  document.body.append(anchor);
  anchor.click();
  anchor.remove();
  URL.revokeObjectURL(url);
}

function renderResponseBlock(
  block: AgentResponseBlockDto,
  graphMap: Record<string, GraphArtifactDto>,
): ReactNode {
  if (block.kind === "markdown") {
    return <pre className="section-markdown">{block.markdown}</pre>;
  }

  if (block.kind === "mermaid") {
    const graph = graphMap[block.graph_id];
    if (!graph) {
      return <div className="graph-empty">Graph artifact `{block.graph_id}` was not found.</div>;
    }

    if (graph.kind !== "mermaid") {
      return <div className="graph-empty">Graph `{graph.title}` is in the graph pane.</div>;
    }

    return (
      <MermaidDiagram syntax={graph.mermaid_syntax} className="mermaid-shell inline-mermaid" />
    );
  }

  return null;
}

export function ResponsePane({
  selectedTab,
  onSelectTab,
  prompt,
  onPromptChange,
  retrievalProfile,
  onRetrievalProfileChange,
  agentBackend,
  onAgentBackendChange,
  agentCommand,
  onAgentCommandChange,
  onAskAgent,
  isBusy,
  projectOpen,
  agentAnswer,
  graphMap,
  onActivateGraph,
  rootSymbols,
  childrenByNode,
  expandedNodes,
  onToggleNode,
  onFocusSymbol,
  activeSymbolId,
}: ResponsePaneProps) {
  const handlePromptChange = (event: ChangeEvent<HTMLTextAreaElement>) => {
    onPromptChange(event.target.value);
  };

  const updateCustomConfig = (patch: Partial<ResolvedCustomConfig>) => {
    const current = asCustomConfig(retrievalProfile);
    onRetrievalProfileChange({
      kind: "custom",
      config: {
        ...current,
        ...patch,
      },
    });
  };

  const [showAdvancedSettings, setShowAdvancedSettings] = useState<boolean>(false);
  const [explorerQuery, setExplorerQuery] = useState<string>("");
  const [hideDependencies, setHideDependencies] = useState<boolean>(true);
  const [collapseDuplicates, setCollapseDuplicates] = useState<boolean>(true);
  const query = explorerQuery.trim().toLowerCase();
  const quickPickValue =
    retrievalProfile.kind === "preset"
      ? `preset:${retrievalProfile.preset}`
      : retrievalProfile.kind;

  const handleQuickPickChange = (event: ChangeEvent<HTMLSelectElement>) => {
    const nextValue = event.target.value;
    if (nextValue === "auto") {
      onRetrievalProfileChange({ kind: "auto" });
      return;
    }
    if (nextValue === "custom") {
      onRetrievalProfileChange({
        kind: "custom",
        config: asCustomConfig(retrievalProfile),
      });
      return;
    }
    if (nextValue.startsWith("preset:")) {
      const preset = nextValue.slice("preset:".length) as AgentRetrievalPresetDto;
      onRetrievalProfileChange({ kind: "preset", preset });
    }
  };

  const matchesQuery = (entry: ExplorerEntry): boolean => {
    if (query.length === 0) {
      return true;
    }

    return (
      entry.displayLabel.toLowerCase().includes(query) ||
      entry.node.kind.toLowerCase().includes(query) ||
      (entry.node.file_path?.toLowerCase().includes(query) ?? false)
    );
  };

  const visibleRootStats = useMemo(() => {
    const entries = buildExplorerEntries(rootSymbols, collapseDuplicates);
    const visible = entries.filter((entry) => {
      if (hideDependencies && entry.isDependency) {
        return false;
      }
      return matchesQuery(entry);
    });

    const hiddenDependencies = hideDependencies
      ? entries.filter((entry) => entry.isDependency).length
      : 0;
    const hiddenDuplicates = collapseDuplicates ? rootSymbols.length - entries.length : 0;

    return {
      visible: visible.length,
      hiddenDependencies,
      hiddenDuplicates,
      totalRaw: rootSymbols.length,
    };
  }, [collapseDuplicates, hideDependencies, query, rootSymbols]);

  const renderTree = (nodes: SymbolSummaryDto[], depth = 0): ReactNode[] => {
    const entries = buildExplorerEntries(nodes, collapseDuplicates);

    return entries.flatMap((entry) => {
      if (hideDependencies && entry.isDependency) {
        return [];
      }

      const node = entry.node;
      const expanded = expandedNodes[node.id] ?? false;
      const children = childrenByNode[node.id] ?? [];
      const hasChildren = node.has_children;
      const childElements = expanded ? renderTree(children, depth + 1) : [];
      const selfMatches = matchesQuery(entry);

      if (query.length > 0 && !selfMatches && childElements.length === 0) {
        return [];
      }

      const current = (
        <div
          key={node.id}
          className={`tree-node ${activeSymbolId === node.id ? "tree-node-active" : ""}`.trim()}
          style={{ paddingLeft: `${depth * 16}px` }}
        >
          <button
            className={`tree-toggle ${hasChildren ? "" : "tree-toggle-empty"}`.trim()}
            onClick={() => {
              if (hasChildren) {
                void onToggleNode(node);
              }
            }}
            aria-label={`${expanded ? "Collapse" : "Expand"} ${entry.displayLabel}`}
          >
            {hasChildren ? (expanded ? "▾" : "▸") : "·"}
          </button>
          <button
            className="tree-label"
            onClick={() => {
              onFocusSymbol(node.id, node.label);
            }}
            title={node.file_path ?? node.label}
          >
            <span className="tree-label-top">
              <span className="kind-pill">{node.kind}</span>
              <span className="tree-name">{entry.displayLabel}</span>
              {entry.duplicateCount > 1 ? (
                <span className="tree-duplicate-pill">x{entry.duplicateCount}</span>
              ) : null}
            </span>
            {node.file_path ? <span className="tree-path">{node.file_path}</span> : null}
          </button>
        </div>
      );

      if (!expanded || childElements.length === 0) {
        return [current];
      }

      return [current, ...childElements];
    });
  };

  const treeRows = renderTree(rootSymbols);
  const activeCustomConfig = asCustomConfig(retrievalProfile);

  return (
    <section className="pane pane-response">
      <div className="pane-header">
        <div className="tabs">
          <button
            className={selectedTab === "agent" ? "tab-active" : ""}
            onClick={() => onSelectTab("agent")}
          >
            Agent
          </button>
          <button
            className={selectedTab === "explorer" ? "tab-active" : ""}
            onClick={() => onSelectTab("explorer")}
          >
            Explorer
          </button>
        </div>
      </div>

      {selectedTab === "agent" ? (
        <>
          <div className="prompt-box">
            <textarea
              value={prompt}
              onChange={handlePromptChange}
              placeholder="Ask about architecture, behavior, or impact."
            />
            <div className="retrieval-profile-settings">
              <label className="agent-connection-field">
                <span>Profile quick pick</span>
                <select value={quickPickValue} onChange={handleQuickPickChange}>
                  <option value="auto">Auto (latency-first)</option>
                  {(Object.keys(PRESET_LABELS) as AgentRetrievalPresetDto[]).map((preset) => (
                    <option key={preset} value={`preset:${preset}`}>
                      {PRESET_LABELS[preset]}
                    </option>
                  ))}
                  <option value="custom">Custom (advanced)</option>
                </select>
              </label>
            </div>

            <div className="prompt-actions">
              <div className="prompt-actions-meta">Add context, then ask.</div>
              <button onClick={onAskAgent} disabled={isBusy || !projectOpen}>
                Ask Agent
              </button>
            </div>

            <AdvancedSettingsDrawer
              isOpen={showAdvancedSettings}
              onToggle={() => {
                setShowAdvancedSettings((previous) => !previous);
              }}
              retrievalProfile={retrievalProfile}
              onRetrievalProfileChange={onRetrievalProfileChange}
              activeCustomConfig={activeCustomConfig}
              onCustomConfigChange={updateCustomConfig}
              agentBackend={agentBackend}
              onAgentBackendChange={onAgentBackendChange}
              agentCommand={agentCommand}
              onAgentCommandChange={onAgentCommandChange}
              retrievalTrace={agentAnswer?.retrieval_trace ?? null}
            />
          </div>

          {agentAnswer ? (
            <div className="response-answer-cards">
              <article className="card response-summary-card">
                <div className="section-card-header">
                  <h3>{agentAnswer.summary}</h3>
                  <div className="section-card-actions">
                    <button
                      type="button"
                      onClick={() => {
                        const baseName = sanitizeFileName(
                          `${agentAnswer.answer_id}-${agentAnswer.summary}`,
                        );
                        triggerTextDownload(
                          `${baseName}.md`,
                          buildMarkdownSummary(agentAnswer, graphMap),
                          "text/markdown",
                        );
                      }}
                    >
                      Export Markdown
                    </button>
                    <button
                      type="button"
                      onClick={() => {
                        const baseName = sanitizeFileName(
                          `${agentAnswer.answer_id}-${agentAnswer.summary}`,
                        );
                        triggerTextDownload(
                          `${baseName}.json`,
                          buildJsonSummary(agentAnswer, graphMap),
                          "application/json",
                        );
                      }}
                    >
                      Export JSON
                    </button>
                  </div>
                </div>
                <p>{agentAnswer.prompt}</p>
              </article>

              <article className="card response-evidence-card">
                <div className="section-card-header">
                  <h4>Evidence</h4>
                </div>
                {agentAnswer.citations.length > 0 ? (
                  <ul className="response-citation-list">
                    {agentAnswer.citations.map((citation) => (
                      <li
                        key={`${citation.node_id}:${citation.line ?? "unknown"}:${citation.display_name}`}
                        className="response-citation-item"
                      >
                        <div className="response-citation-meta">
                          <strong>{citation.display_name}</strong>
                          <span>{citation.kind}</span>
                          <span>{citationLocationLabel(citation)}</span>
                          <span>Score {citation.score.toFixed(3)}</span>
                        </div>
                        <button
                          type="button"
                          onClick={() => {
                            onFocusSymbol(citation.node_id, citation.display_name);
                          }}
                        >
                          Jump to Cited Node
                        </button>
                      </li>
                    ))}
                  </ul>
                ) : (
                  <div className="graph-empty">No citations yet. Ask a narrower question.</div>
                )}
              </article>

              {agentAnswer.sections.map((section) => (
                <article key={section.id} className="card section-block">
                  <div className="section-card-header">
                    <h4>{section.title}</h4>
                    <div className="section-card-actions">
                      {sectionGraphs(section, graphMap).map((graph) => (
                        <button
                          key={`${section.id}-${graph.id}`}
                          type="button"
                          onClick={() => {
                            onActivateGraph(graph.id);
                          }}
                        >
                          Open Related Graph: {graph.title}
                        </button>
                      ))}
                    </div>
                  </div>
                  <div className="section-block-content">
                    {section.blocks.map((block, index) => (
                      <div key={`${section.id}-${index}`} className="response-block">
                        {renderResponseBlock(block, graphMap)}
                      </div>
                    ))}
                  </div>
                </article>
              ))}
            </div>
          ) : (
            <div className="card response-empty-state">
              <h3>Ask your first question</h3>
              <p>Run the current prompt, then inspect evidence and graphs.</p>
              <div className="graph-links">
                <button type="button" onClick={onAskAgent} disabled={isBusy || !projectOpen}>
                  Ask With Current Prompt
                </button>
              </div>
            </div>
          )}
        </>
      ) : (
        <div className="card explorer-card">
          <h3>Symbol Explorer</h3>
          <p>Search symbols, then focus one to inspect code.</p>
          <div className="explorer-toolbar">
            <input
              className="explorer-search-input"
              value={explorerQuery}
              onChange={(event) => setExplorerQuery(event.target.value)}
              placeholder="Filter symbols, kinds, or files"
              aria-label="Filter explorer symbols"
            />
            <details>
              <summary>Explorer options</summary>
              <div className="explorer-toolbar-row">
                <label>
                  <input
                    type="checkbox"
                    checked={hideDependencies}
                    onChange={(event) => setHideDependencies(event.target.checked)}
                  />
                  Hide dependencies
                </label>
                <label>
                  <input
                    type="checkbox"
                    checked={collapseDuplicates}
                    onChange={(event) => setCollapseDuplicates(event.target.checked)}
                  />
                  Collapse duplicates
                </label>
                <button
                  type="button"
                  className="explorer-clear-button"
                  onClick={() => setExplorerQuery("")}
                  disabled={explorerQuery.length === 0}
                >
                  Clear
                </button>
              </div>
            </details>
            <div className="explorer-summary">
              <span>{visibleRootStats.visible} shown</span>
              <span>{visibleRootStats.totalRaw} total</span>
              {visibleRootStats.hiddenDependencies > 0 ? (
                <span>{visibleRootStats.hiddenDependencies} dependencies hidden</span>
              ) : null}
              {visibleRootStats.hiddenDuplicates > 0 ? (
                <span>{visibleRootStats.hiddenDuplicates} duplicates collapsed</span>
              ) : null}
            </div>
          </div>
          <div className="tree-root">
            {treeRows.length > 0 ? (
              treeRows
            ) : (
              <div className="explorer-empty">No matches. Clear filters or try another term.</div>
            )}
          </div>
        </div>
      )}
    </section>
  );
}
