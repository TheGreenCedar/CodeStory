import type {
  EdgeKind,
  NodeKind,
  TrailConfigDto,
  TrailDirection,
  TrailMode,
} from "../generated/api";

export const EDGE_KIND_OPTIONS: EdgeKind[] = [
  "MEMBER",
  "TYPE_USAGE",
  "USAGE",
  "CALL",
  "INHERITANCE",
  "OVERRIDE",
  "TYPE_ARGUMENT",
  "TEMPLATE_SPECIALIZATION",
  "INCLUDE",
  "IMPORT",
  "MACRO_USAGE",
  "ANNOTATION_USAGE",
  "UNKNOWN",
];

export const NODE_KIND_OPTIONS: NodeKind[] = [
  "MODULE",
  "NAMESPACE",
  "PACKAGE",
  "FILE",
  "STRUCT",
  "CLASS",
  "INTERFACE",
  "ANNOTATION",
  "UNION",
  "ENUM",
  "TYPEDEF",
  "TYPE_PARAMETER",
  "BUILTIN_TYPE",
  "FUNCTION",
  "METHOD",
  "MACRO",
  "GLOBAL_VARIABLE",
  "FIELD",
  "VARIABLE",
  "CONSTANT",
  "ENUM_CONSTANT",
  "UNKNOWN",
];

export type TrailUiConfig = {
  mode: TrailMode;
  targetId: string | null;
  targetLabel: string;
  depth: number;
  direction: TrailDirection;
  edgeFilter: EdgeKind[];
  nodeFilter: NodeKind[];
  maxNodes: number;
};

export type PersistedTrailUiConfig = Partial<TrailUiConfig>;

export function defaultTrailUiConfig(): TrailUiConfig {
  return {
    mode: "Neighborhood",
    targetId: null,
    targetLabel: "",
    depth: 2,
    direction: "Both",
    edgeFilter: [...EDGE_KIND_OPTIONS],
    nodeFilter: [],
    maxNodes: 500,
  };
}

function clampDepth(value: number): number {
  if (!Number.isFinite(value)) {
    return 2;
  }
  if (value < 0) {
    return 0;
  }
  return Math.min(Math.round(value), 64);
}

function clampMaxNodes(value: number): number {
  if (!Number.isFinite(value)) {
    return 500;
  }
  return Math.max(10, Math.min(100_000, Math.round(value)));
}

export function normalizeTrailUiConfig(
  raw: PersistedTrailUiConfig | null | undefined,
): TrailUiConfig {
  const defaults = defaultTrailUiConfig();
  if (!raw || typeof raw !== "object") {
    return defaults;
  }

  const mode =
    raw.mode === "Neighborhood" ||
    raw.mode === "AllReferenced" ||
    raw.mode === "AllReferencing" ||
    raw.mode === "ToTargetSymbol"
      ? raw.mode
      : defaults.mode;

  const direction =
    raw.direction === "Incoming" || raw.direction === "Outgoing" || raw.direction === "Both"
      ? raw.direction
      : defaults.direction;

  const edgeFilter = Array.isArray(raw.edgeFilter)
    ? raw.edgeFilter.filter((kind): kind is EdgeKind =>
        EDGE_KIND_OPTIONS.includes(kind as EdgeKind),
      )
    : defaults.edgeFilter;

  const nodeFilter = Array.isArray(raw.nodeFilter)
    ? raw.nodeFilter.filter((kind): kind is NodeKind =>
        NODE_KIND_OPTIONS.includes(kind as NodeKind),
      )
    : defaults.nodeFilter;

  return {
    mode,
    targetId: typeof raw.targetId === "string" ? raw.targetId : null,
    targetLabel: typeof raw.targetLabel === "string" ? raw.targetLabel : "",
    depth: clampDepth(typeof raw.depth === "number" ? raw.depth : defaults.depth),
    direction,
    edgeFilter: edgeFilter.length > 0 ? edgeFilter : defaults.edgeFilter,
    nodeFilter,
    maxNodes: clampMaxNodes(typeof raw.maxNodes === "number" ? raw.maxNodes : defaults.maxNodes),
  };
}

export function toTrailConfigDto(rootId: string, config: TrailUiConfig): TrailConfigDto {
  return {
    root_id: rootId,
    mode: config.mode,
    target_id: config.mode === "ToTargetSymbol" ? config.targetId : null,
    depth: clampDepth(config.depth),
    direction: config.direction,
    edge_filter: config.edgeFilter,
    node_filter: config.nodeFilter,
    max_nodes: clampMaxNodes(config.maxNodes),
  };
}
