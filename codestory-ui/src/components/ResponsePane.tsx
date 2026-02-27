import { useMemo, useState, type ChangeEvent, type ReactNode } from "react";

import type { LeftTab } from "../app/types";
import { MermaidDiagram } from "./MermaidDiagram";
import type {
  AgentAnswerDto,
  AgentConnectionSettingsDto,
  AgentResponseBlockDto,
  AgentRetrievalProfileSelectionDto,
  EdgeKind,
  GraphArtifactDto,
  NodeKind,
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

type ResolvedCustomConfig = {
  depth: number;
  direction: "Incoming" | "Outgoing" | "Both";
  edge_filter: EdgeKind[];
  node_filter: NodeKind[];
  max_nodes: number;
  include_edge_occurrences: boolean;
  enable_source_reads: boolean;
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

const PRESET_LABELS: Record<"architecture" | "callflow" | "inheritance" | "impact", string> = {
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

function parseCsvList(value: string): string[] {
  return value
    .split(",")
    .map((entry) => entry.trim())
    .filter((entry) => entry.length > 0);
}

function formatCsvList(values: string[]): string {
  return values.join(", ");
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

function renderResponseBlock(
  block: AgentResponseBlockDto,
  graphMap: Record<string, GraphArtifactDto>,
  onActivateGraph: (graphId: string) => void,
): ReactNode {
  if (block.kind === "markdown") {
    return <pre className="section-markdown">{block.markdown}</pre>;
  }

  if (block.kind === "mermaid") {
    const graph = graphMap[block.graph_id];
    if (!graph) {
      return (
        <div className="graph-empty">
          Mermaid artifact `{block.graph_id}` was not found in this response payload.
        </div>
      );
    }

    if (graph.kind !== "mermaid") {
      return (
        <div className="graph-links">
          <button onClick={() => onActivateGraph(graph.id)}>Open {graph.title}</button>
        </div>
      );
    }

    return (
      <div className="inline-mermaid-block">
        <div className="graph-links">
          <button onClick={() => onActivateGraph(graph.id)}>Open in Graph Pane</button>
        </div>
        <MermaidDiagram syntax={graph.mermaid_syntax} className="mermaid-shell inline-mermaid" />
      </div>
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

  const handleBackendChange = (event: ChangeEvent<HTMLSelectElement>) => {
    const nextBackend = event.target.value;
    if (nextBackend === "codex" || nextBackend === "claude_code") {
      onAgentBackendChange(nextBackend);
    }
  };

  const handleCommandChange = (event: ChangeEvent<HTMLInputElement>) => {
    onAgentCommandChange(event.target.value);
  };

  const handleProfileModeChange = (event: ChangeEvent<HTMLSelectElement>) => {
    const nextMode = event.target.value;
    if (nextMode === "auto") {
      onRetrievalProfileChange({ kind: "auto" });
      return;
    }
    if (nextMode === "preset") {
      const preset = retrievalProfile.kind === "preset" ? retrievalProfile.preset : "architecture";
      onRetrievalProfileChange({ kind: "preset", preset });
      return;
    }
    if (nextMode === "custom") {
      onRetrievalProfileChange({
        kind: "custom",
        config: asCustomConfig(retrievalProfile),
      });
    }
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

  const [explorerQuery, setExplorerQuery] = useState<string>("");
  const [hideDependencies, setHideDependencies] = useState<boolean>(true);
  const [collapseDuplicates, setCollapseDuplicates] = useState<boolean>(true);
  const query = explorerQuery.trim().toLowerCase();

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
              placeholder="Ask Codestory to explain architecture, trace behavior, or summarize relationships"
            />
            <div className="agent-connection-settings">
              <label className="agent-connection-field">
                <span>Local agent</span>
                <select value={agentBackend} onChange={handleBackendChange}>
                  <option value="codex">Codex</option>
                  <option value="claude_code">Claude Code</option>
                </select>
              </label>
              <label className="agent-connection-field">
                <span>Command override (optional)</span>
                <input
                  value={agentCommand}
                  onChange={handleCommandChange}
                  placeholder="Executable path or command name"
                />
              </label>
            </div>

            <div className="retrieval-profile-settings">
              <label className="agent-connection-field">
                <span>Retrieval profile</span>
                <select value={retrievalProfile.kind} onChange={handleProfileModeChange}>
                  <option value="auto">Auto (latency-first)</option>
                  <option value="preset">Preset (latency-first)</option>
                  <option value="custom">Custom (completeness-first)</option>
                </select>
              </label>

              {retrievalProfile.kind === "preset" ? (
                <label className="agent-connection-field">
                  <span>Preset</span>
                  <select
                    value={retrievalProfile.preset}
                    onChange={(event) => {
                      const nextPreset = event.target.value as
                        | "architecture"
                        | "callflow"
                        | "inheritance"
                        | "impact";
                      onRetrievalProfileChange({ kind: "preset", preset: nextPreset });
                    }}
                  >
                    {(Object.keys(PRESET_LABELS) as Array<keyof typeof PRESET_LABELS>).map(
                      (preset) => (
                        <option key={preset} value={preset}>
                          {PRESET_LABELS[preset]}
                        </option>
                      ),
                    )}
                  </select>
                </label>
              ) : null}

              {retrievalProfile.kind === "custom" ? (
                <div className="custom-profile-grid">
                  <label className="agent-connection-field">
                    <span>Depth (0 = infinite)</span>
                    <input
                      type="number"
                      min={0}
                      value={activeCustomConfig.depth}
                      onChange={(event) => {
                        const nextDepth = Number.parseInt(event.target.value, 10);
                        updateCustomConfig({
                          depth: Number.isFinite(nextDepth) ? Math.max(0, nextDepth) : 0,
                        });
                      }}
                    />
                  </label>

                  <label className="agent-connection-field">
                    <span>Direction</span>
                    <select
                      value={activeCustomConfig.direction}
                      onChange={(event) => {
                        const nextDirection = event.target.value;
                        updateCustomConfig({
                          direction:
                            nextDirection === "Incoming" || nextDirection === "Outgoing"
                              ? nextDirection
                              : "Both",
                        });
                      }}
                    >
                      <option value="Both">Both</option>
                      <option value="Outgoing">Outgoing</option>
                      <option value="Incoming">Incoming</option>
                    </select>
                  </label>

                  <label className="agent-connection-field">
                    <span>Max nodes</span>
                    <input
                      type="number"
                      min={10}
                      value={activeCustomConfig.max_nodes}
                      onChange={(event) => {
                        const nextMaxNodes = Number.parseInt(event.target.value, 10);
                        updateCustomConfig({
                          max_nodes: Number.isFinite(nextMaxNodes)
                            ? Math.max(10, nextMaxNodes)
                            : activeCustomConfig.max_nodes,
                        });
                      }}
                    />
                  </label>

                  <label className="agent-connection-field agent-connection-field-wide">
                    <span>Edge filter (comma-separated)</span>
                    <input
                      value={formatCsvList(activeCustomConfig.edge_filter)}
                      onChange={(event) => {
                        updateCustomConfig({
                          edge_filter: parseCsvList(event.target.value) as EdgeKind[],
                        });
                      }}
                      placeholder="CALL, INHERITANCE, OVERRIDE"
                    />
                  </label>

                  <label className="agent-connection-field agent-connection-field-wide">
                    <span>Node filter (comma-separated)</span>
                    <input
                      value={formatCsvList(activeCustomConfig.node_filter)}
                      onChange={(event) => {
                        updateCustomConfig({
                          node_filter: parseCsvList(event.target.value) as NodeKind[],
                        });
                      }}
                      placeholder="CLASS, METHOD, INTERFACE"
                    />
                  </label>

                  <label className="profile-checkbox">
                    <input
                      type="checkbox"
                      checked={activeCustomConfig.enable_source_reads}
                      onChange={(event) => {
                        updateCustomConfig({ enable_source_reads: event.target.checked });
                      }}
                    />
                    Enable optional source reads after retrieval
                  </label>

                  <label className="profile-checkbox">
                    <input
                      type="checkbox"
                      checked={activeCustomConfig.include_edge_occurrences}
                      onChange={(event) => {
                        updateCustomConfig({ include_edge_occurrences: event.target.checked });
                      }}
                    />
                    Include edge occurrence lookups
                  </label>
                </div>
              ) : null}
            </div>

            <div className="prompt-actions">
              <div className="prompt-actions-meta">
                Mermaid diagrams are always included and rendered inline.
              </div>
              <button onClick={onAskAgent} disabled={isBusy || !projectOpen}>
                Ask Agent
              </button>
            </div>
          </div>

          {agentAnswer && (
            <div className="card">
              <h3>{agentAnswer.summary}</h3>
              {agentAnswer.sections.map((section) => (
                <article key={section.id} className="section-block">
                  <h4>{section.title}</h4>
                  <div className="section-block-content">
                    {section.blocks.map((block, index) => (
                      <div key={`${section.id}-${index}`} className="response-block">
                        {renderResponseBlock(block, graphMap, onActivateGraph)}
                      </div>
                    ))}
                  </div>
                </article>
              ))}

              <details className="trace-panel">
                <summary>Retrieval Trace</summary>
                <pre>{JSON.stringify(agentAnswer.retrieval_trace, null, 2)}</pre>
              </details>
            </div>
          )}
        </>
      ) : (
        <div className="card explorer-card">
          <h3>Symbol Explorer</h3>
          <p>Browse the indexed symbol tree without asking a prompt.</p>
          <div className="explorer-toolbar">
            <input
              className="explorer-search-input"
              value={explorerQuery}
              onChange={(event) => setExplorerQuery(event.target.value)}
              placeholder="Filter symbols, kinds, or files"
              aria-label="Filter explorer symbols"
            />
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
            <div className="explorer-summary">
              <span>{visibleRootStats.visible} visible roots</span>
              <span>{visibleRootStats.totalRaw} total roots</span>
              {visibleRootStats.hiddenDependencies > 0 ? (
                <span>{visibleRootStats.hiddenDependencies} dependency roots hidden</span>
              ) : null}
              {visibleRootStats.hiddenDuplicates > 0 ? (
                <span>{visibleRootStats.hiddenDuplicates} duplicate roots collapsed</span>
              ) : null}
            </div>
          </div>
          <div className="tree-root">
            {treeRows.length > 0 ? (
              treeRows
            ) : (
              <div className="explorer-empty">No symbols match the current filters.</div>
            )}
          </div>
        </div>
      )}
    </section>
  );
}
