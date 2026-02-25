import { type ChangeEvent, type ReactNode } from "react";

import type { LeftTab } from "../app/types";
import type { AgentAnswerDto, GraphArtifactDto, SymbolSummaryDto } from "../generated/api";

type ResponsePaneProps = {
  selectedTab: LeftTab;
  onSelectTab: (tab: LeftTab) => void;
  prompt: string;
  onPromptChange: (prompt: string) => void;
  includeMermaid: boolean;
  onIncludeMermaidChange: (next: boolean) => void;
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
};

export function ResponsePane({
  selectedTab,
  onSelectTab,
  prompt,
  onPromptChange,
  includeMermaid,
  onIncludeMermaidChange,
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
}: ResponsePaneProps) {
  const handlePromptChange = (event: ChangeEvent<HTMLTextAreaElement>) => {
    onPromptChange(event.target.value);
  };

  const renderTree = (nodes: SymbolSummaryDto[], depth = 0): ReactNode[] => {
    return nodes.flatMap((node) => {
      const expanded = expandedNodes[node.id] ?? false;
      const children = childrenByNode[node.id] ?? [];
      const hasChildren = node.has_children;

      const current = (
        <div key={node.id} className="tree-node" style={{ paddingLeft: `${depth * 16}px` }}>
          <button
            className={`tree-toggle ${hasChildren ? "" : "tree-toggle-empty"}`.trim()}
            onClick={() => {
              if (hasChildren) {
                void onToggleNode(node);
              }
            }}
            aria-label={expanded ? "Collapse" : "Expand"}
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
            <span className="kind-pill">{node.kind}</span>
            <span>{node.label}</span>
          </button>
        </div>
      );

      if (!expanded || children.length === 0) {
        return [current];
      }

      return [current, ...renderTree(children, depth + 1)];
    });
  };

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
            <div className="prompt-actions">
              <label>
                <input
                  type="checkbox"
                  checked={includeMermaid}
                  onChange={(event) => onIncludeMermaidChange(event.target.checked)}
                />
                Add Mermaid diagrams
              </label>
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
                  <pre>{section.markdown}</pre>
                  <div className="graph-links">
                    {section.graph_ids.map((graphId) => (
                      <button key={graphId} onClick={() => onActivateGraph(graphId)}>
                        {graphMap[graphId]?.title ?? graphId}
                      </button>
                    ))}
                  </div>
                </article>
              ))}
            </div>
          )}
        </>
      ) : (
        <div className="card explorer-card">
          <h3>Symbol Explorer</h3>
          <p>Browse the indexed symbol tree without asking a prompt.</p>
          <div className="tree-root">{renderTree(rootSymbols)}</div>
        </div>
      )}
    </section>
  );
}
