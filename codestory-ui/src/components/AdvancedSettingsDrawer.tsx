import type { ChangeEvent } from "react";

import type {
  AgentConnectionSettingsDto,
  AgentRetrievalPresetDto,
  AgentRetrievalProfileSelectionDto,
  AgentRetrievalTraceDto,
  EdgeKind,
  NodeKind,
} from "../generated/api";

export type ResolvedCustomConfig = {
  depth: number;
  direction: "Incoming" | "Outgoing" | "Both";
  edge_filter: EdgeKind[];
  node_filter: NodeKind[];
  max_nodes: number;
  include_edge_occurrences: boolean;
  enable_source_reads: boolean;
};

const PRESET_LABELS: Record<AgentRetrievalPresetDto, string> = {
  architecture: "Architecture",
  callflow: "Call Flow",
  inheritance: "Inheritance",
  impact: "Impact",
};

type AdvancedSettingsDrawerProps = {
  isOpen: boolean;
  onToggle: () => void;
  retrievalProfile: AgentRetrievalProfileSelectionDto;
  onRetrievalProfileChange: (next: AgentRetrievalProfileSelectionDto) => void;
  activeCustomConfig: ResolvedCustomConfig;
  onCustomConfigChange: (patch: Partial<ResolvedCustomConfig>) => void;
  agentBackend: NonNullable<AgentConnectionSettingsDto["backend"]>;
  onAgentBackendChange: (backend: NonNullable<AgentConnectionSettingsDto["backend"]>) => void;
  agentCommand: string;
  onAgentCommandChange: (command: string) => void;
  retrievalTrace: AgentRetrievalTraceDto | null;
};

function parseCsvList(value: string): string[] {
  return value
    .split(",")
    .map((entry) => entry.trim())
    .filter((entry) => entry.length > 0);
}

function formatCsvList(values: string[]): string {
  return values.join(", ");
}

export function AdvancedSettingsDrawer({
  isOpen,
  onToggle,
  retrievalProfile,
  onRetrievalProfileChange,
  activeCustomConfig,
  onCustomConfigChange,
  agentBackend,
  onAgentBackendChange,
  agentCommand,
  onAgentCommandChange,
  retrievalTrace,
}: AdvancedSettingsDrawerProps) {
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
        config: activeCustomConfig,
      });
    }
  };

  return (
    <div className="advanced-settings">
      <button
        type="button"
        className="advanced-settings-toggle"
        onClick={onToggle}
        aria-expanded={isOpen}
      >
        {isOpen ? "Hide Advanced Settings" : "Show Advanced Settings"}
      </button>

      {isOpen ? (
        <div className="advanced-settings-drawer">
          <div className="agent-connection-settings">
            <label className="agent-connection-field">
              <span>Agent</span>
              <select value={agentBackend} onChange={handleBackendChange}>
                <option value="codex">Codex</option>
                <option value="claude_code">Claude Code</option>
              </select>
            </label>
            <label className="agent-connection-field">
              <span>Command override</span>
              <input
                value={agentCommand}
                onChange={handleCommandChange}
                placeholder="Optional executable path"
              />
            </label>
          </div>

          <div className="retrieval-profile-settings">
            <label className="agent-connection-field">
              <span>Retrieval mode</span>
              <select value={retrievalProfile.kind} onChange={handleProfileModeChange}>
                <option value="auto">Auto</option>
                <option value="preset">Preset</option>
                <option value="custom">Custom</option>
              </select>
            </label>

            {retrievalProfile.kind === "preset" ? (
              <label className="agent-connection-field">
                <span>Preset</span>
                <select
                  value={retrievalProfile.preset}
                  onChange={(event) => {
                    const nextPreset = event.target.value as AgentRetrievalPresetDto;
                    onRetrievalProfileChange({ kind: "preset", preset: nextPreset });
                  }}
                >
                  {(Object.keys(PRESET_LABELS) as AgentRetrievalPresetDto[]).map((preset) => (
                    <option key={preset} value={preset}>
                      {PRESET_LABELS[preset]}
                    </option>
                  ))}
                </select>
              </label>
            ) : null}

            {retrievalProfile.kind === "custom" ? (
              <div className="custom-profile-grid">
                <label className="agent-connection-field">
                  <span>Depth (0 = unlimited)</span>
                  <input
                    type="number"
                    min={0}
                    value={activeCustomConfig.depth}
                    onChange={(event) => {
                      const nextDepth = Number.parseInt(event.target.value, 10);
                      onCustomConfigChange({
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
                      onCustomConfigChange({
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
                      onCustomConfigChange({
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
                      onCustomConfigChange({
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
                      onCustomConfigChange({
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
                      onCustomConfigChange({ enable_source_reads: event.target.checked });
                    }}
                  />
                  Read source after retrieval
                </label>

                <label className="profile-checkbox">
                  <input
                    type="checkbox"
                    checked={activeCustomConfig.include_edge_occurrences}
                    onChange={(event) => {
                      onCustomConfigChange({ include_edge_occurrences: event.target.checked });
                    }}
                  />
                  Include edge occurrence lookups
                </label>
              </div>
            ) : null}
          </div>

          <details className="trace-panel">
            <summary>Raw Retrieval Trace</summary>
            {retrievalTrace ? (
              <pre>{JSON.stringify(retrievalTrace, null, 2)}</pre>
            ) : (
              <div className="graph-empty">Ask a question to view trace data.</div>
            )}
          </details>
        </div>
      ) : null}
    </div>
  );
}
