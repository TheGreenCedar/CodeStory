import type {
  AgentConnectionSettingsDto,
  AgentCustomRetrievalConfigDto,
  AgentRetrievalProfileSelectionDto,
} from "../generated/api";
import type { TrailUiConfig } from "../graph/trailConfig";

export function toMonacoModelPath(path: string | null): string | null {
  if (!path) {
    return null;
  }

  const forwardSlashPath = path.replace(/\\/g, "/");
  return forwardSlashPath.replace(/^([A-Za-z]:)/, "/$1");
}

export function isLikelyTestOrBenchPath(path: string | null): boolean {
  if (!path) {
    return false;
  }
  const normalized = path.replace(/\\/g, "/").toLowerCase();
  const segments = normalized.split("/").filter((segment) => segment.length > 0);
  return (
    segments.includes("tests") ||
    segments.includes("test") ||
    segments.includes("benches") ||
    segments.includes("bench") ||
    normalized.endsWith("_test.rs") ||
    normalized.includes(".test.") ||
    normalized.includes(".spec.")
  );
}

export type FocusGraphMode = "neighborhood" | "trailDepthOne";

export function trailModeLabel(mode: TrailUiConfig["mode"]): string {
  if (mode === "Neighborhood") {
    return "Neighborhood";
  }
  if (mode === "AllReferenced") {
    return "All Referenced";
  }
  if (mode === "AllReferencing") {
    return "All Referencing";
  }
  return "To Target";
}

export type AgentConnectionState = {
  backend: NonNullable<AgentConnectionSettingsDto["backend"]>;
  command: string | null;
};

export const UI_LAYOUT_SCHEMA_VERSION = 2;
export const LAST_OPENED_PROJECT_KEY = "codestory:last-opened-project";

export const DEFAULT_AGENT_CONNECTION: AgentConnectionState = {
  backend: "codex",
  command: null,
};

export const DEFAULT_RETRIEVAL_PROFILE: AgentRetrievalProfileSelectionDto = {
  kind: "auto",
};

const ALLOWED_PRESETS = new Set(["architecture", "callflow", "inheritance", "impact"]);

export function normalizeAgentConnection(raw: unknown): AgentConnectionState {
  if (!raw || typeof raw !== "object") {
    return DEFAULT_AGENT_CONNECTION;
  }

  const candidate = raw as Partial<AgentConnectionSettingsDto>;
  const backend = candidate.backend === "claude_code" ? "claude_code" : "codex";
  const command =
    typeof candidate.command === "string" && candidate.command.trim().length > 0
      ? candidate.command.trim()
      : null;
  return {
    backend,
    command,
  };
}

export function normalizeCustomConfig(raw: unknown): AgentCustomRetrievalConfigDto {
  if (!raw || typeof raw !== "object") {
    return {
      depth: 3,
      direction: "Both",
      edge_filter: [],
      node_filter: [],
      max_nodes: 800,
      include_edge_occurrences: false,
      enable_source_reads: true,
    };
  }

  const candidate = raw as Partial<AgentCustomRetrievalConfigDto>;
  const depth = typeof candidate.depth === "number" ? Math.max(0, Math.trunc(candidate.depth)) : 3;
  const maxNodes =
    typeof candidate.max_nodes === "number" ? Math.max(10, Math.trunc(candidate.max_nodes)) : 800;
  const direction =
    candidate.direction === "Incoming" || candidate.direction === "Outgoing"
      ? candidate.direction
      : "Both";
  return {
    depth,
    direction,
    edge_filter: Array.isArray(candidate.edge_filter) ? candidate.edge_filter : [],
    node_filter: Array.isArray(candidate.node_filter) ? candidate.node_filter : [],
    max_nodes: maxNodes,
    include_edge_occurrences: Boolean(candidate.include_edge_occurrences),
    enable_source_reads:
      typeof candidate.enable_source_reads === "boolean" ? candidate.enable_source_reads : true,
  };
}

export function normalizeRetrievalProfile(raw: unknown): AgentRetrievalProfileSelectionDto {
  if (!raw || typeof raw !== "object") {
    return DEFAULT_RETRIEVAL_PROFILE;
  }

  const candidate = raw as Partial<AgentRetrievalProfileSelectionDto> & {
    preset?: unknown;
    config?: unknown;
  };

  if (candidate.kind === "preset") {
    const preset = typeof candidate.preset === "string" ? candidate.preset : "";
    if (ALLOWED_PRESETS.has(preset)) {
      return {
        kind: "preset",
        preset: preset as "architecture" | "callflow" | "inheritance" | "impact",
      };
    }
    return {
      kind: "preset",
      preset: "architecture",
    };
  }

  if (candidate.kind === "custom") {
    return {
      kind: "custom",
      config: normalizeCustomConfig(candidate.config),
    };
  }

  return DEFAULT_RETRIEVAL_PROFILE;
}
