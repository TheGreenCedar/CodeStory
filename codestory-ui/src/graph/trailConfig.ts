import type {
  EdgeKind,
  LayoutDirection,
  NodeKind,
  TrailCallerScope,
  TrailConfigDto,
  TrailDirection,
  TrailMode,
} from "../generated/api";

export type GroupingMode = "none" | "namespace" | "file";
export type TrailPerspectivePreset = "Architecture" | "CallFlow" | "Impact" | "Ownership";

export const TRAIL_PERSPECTIVE_PRESETS: TrailPerspectivePreset[] = [
  "Architecture",
  "CallFlow",
  "Impact",
  "Ownership",
];

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
  layoutDirection: LayoutDirection;
  direction: TrailDirection;
  callerScope: TrailCallerScope;
  edgeFilter: EdgeKind[];
  showUtilityCalls: boolean;
  nodeFilter: NodeKind[];
  showLegend: boolean;
  showMiniMap: boolean;
  bundleEdges: boolean;
  groupingMode: GroupingMode;
  maxNodes: number;
};

export type PersistedTrailUiConfig = Partial<TrailUiConfig>;

export function defaultTrailUiConfig(): TrailUiConfig {
  return {
    mode: "Neighborhood",
    targetId: null,
    targetLabel: "",
    depth: 1,
    layoutDirection: "Horizontal",
    direction: "Both",
    callerScope: "ProductionOnly",
    edgeFilter: [...EDGE_KIND_OPTIONS],
    showUtilityCalls: false,
    nodeFilter: [],
    showLegend: true,
    showMiniMap: true,
    bundleEdges: true,
    groupingMode: "none",
    maxNodes: 500,
  };
}

const TRAIL_PERSPECTIVE_PRESET_OVERRIDES: Record<TrailPerspectivePreset, Partial<TrailUiConfig>> = {
  Architecture: {
    mode: "Neighborhood",
    depth: 2,
    layoutDirection: "Horizontal",
    direction: "Both",
    callerScope: "ProductionOnly",
    edgeFilter: [
      "MEMBER",
      "CALL",
      "INHERITANCE",
      "OVERRIDE",
      "TYPE_USAGE",
      "TYPE_ARGUMENT",
      "TEMPLATE_SPECIALIZATION",
      "IMPORT",
      "INCLUDE",
    ],
    nodeFilter: [],
    showUtilityCalls: false,
    bundleEdges: true,
    groupingMode: "namespace",
    maxNodes: 600,
  },
  CallFlow: {
    mode: "Neighborhood",
    depth: 4,
    layoutDirection: "Vertical",
    direction: "Outgoing",
    callerScope: "ProductionOnly",
    edgeFilter: ["CALL", "OVERRIDE", "MACRO_USAGE"],
    nodeFilter: ["CLASS", "STRUCT", "INTERFACE", "FUNCTION", "METHOD", "MACRO"],
    showUtilityCalls: true,
    bundleEdges: false,
    groupingMode: "none",
    maxNodes: 900,
  },
  Impact: {
    mode: "AllReferencing",
    depth: 3,
    layoutDirection: "Horizontal",
    direction: "Incoming",
    callerScope: "IncludeTestsAndBenches",
    edgeFilter: ["CALL", "USAGE", "TYPE_USAGE", "IMPORT", "INCLUDE", "ANNOTATION_USAGE"],
    nodeFilter: [],
    showUtilityCalls: true,
    bundleEdges: true,
    groupingMode: "none",
    maxNodes: 1_500,
  },
  Ownership: {
    mode: "Neighborhood",
    depth: 2,
    layoutDirection: "Horizontal",
    direction: "Both",
    callerScope: "IncludeTestsAndBenches",
    edgeFilter: ["MEMBER", "CALL", "USAGE", "IMPORT", "INCLUDE"],
    nodeFilter: [
      "PACKAGE",
      "MODULE",
      "NAMESPACE",
      "FILE",
      "CLASS",
      "STRUCT",
      "INTERFACE",
      "FUNCTION",
      "METHOD",
    ],
    showUtilityCalls: false,
    bundleEdges: true,
    groupingMode: "file",
    maxNodes: 800,
  },
};

export function trailConfigFromPerspectivePreset(preset: TrailPerspectivePreset): TrailUiConfig {
  const defaults = defaultTrailUiConfig();
  const merged = normalizeTrailUiConfig({
    ...defaults,
    ...TRAIL_PERSPECTIVE_PRESET_OVERRIDES[preset],
  });
  if (merged.mode !== "ToTargetSymbol") {
    return {
      ...merged,
      targetId: null,
      targetLabel: "",
      edgeFilter: [...merged.edgeFilter],
      nodeFilter: [...merged.nodeFilter],
    };
  }
  return {
    ...merged,
    edgeFilter: [...merged.edgeFilter],
    nodeFilter: [...merged.nodeFilter],
  };
}

function sameFilterSet(left: string[], right: string[]): boolean {
  if (left.length !== right.length) {
    return false;
  }
  const rightSet = new Set(right);
  return left.every((entry) => rightSet.has(entry));
}

export function trailPerspectivePresetForConfig(
  config: TrailUiConfig,
): TrailPerspectivePreset | null {
  for (const preset of TRAIL_PERSPECTIVE_PRESETS) {
    const presetConfig = trailConfigFromPerspectivePreset(preset);
    if (
      config.mode === presetConfig.mode &&
      config.depth === presetConfig.depth &&
      config.layoutDirection === presetConfig.layoutDirection &&
      config.direction === presetConfig.direction &&
      config.callerScope === presetConfig.callerScope &&
      config.showUtilityCalls === presetConfig.showUtilityCalls &&
      config.bundleEdges === presetConfig.bundleEdges &&
      config.groupingMode === presetConfig.groupingMode &&
      config.maxNodes === presetConfig.maxNodes &&
      sameFilterSet(config.edgeFilter, presetConfig.edgeFilter) &&
      sameFilterSet(config.nodeFilter, presetConfig.nodeFilter)
    ) {
      return preset;
    }
  }
  return null;
}

function clampDepth(value: number): number {
  if (!Number.isFinite(value)) {
    return 1;
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
  const layoutDirection =
    raw.layoutDirection === "Horizontal" || raw.layoutDirection === "Vertical"
      ? raw.layoutDirection
      : defaults.layoutDirection;

  const callerScope =
    raw.callerScope === "ProductionOnly" || raw.callerScope === "IncludeTestsAndBenches"
      ? raw.callerScope
      : defaults.callerScope;

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

  // Legacy persisted layouts may include bundlingMode; keep ignoring that shape.
  const _legacyBundlingMode = (raw as { bundlingMode?: unknown }).bundlingMode;
  void _legacyBundlingMode;
  const _legacyDebugParityChannels = (raw as { debugParityChannels?: unknown }).debugParityChannels;
  const _legacyDebugParityRoutes = (raw as { debugParityRoutes?: unknown }).debugParityRoutes;
  void _legacyDebugParityChannels;
  void _legacyDebugParityRoutes;

  const groupingMode =
    raw.groupingMode === "namespace" || raw.groupingMode === "file" || raw.groupingMode === "none"
      ? raw.groupingMode
      : defaults.groupingMode;

  return {
    mode,
    targetId: typeof raw.targetId === "string" ? raw.targetId : null,
    targetLabel: typeof raw.targetLabel === "string" ? raw.targetLabel : "",
    depth: clampDepth(typeof raw.depth === "number" ? raw.depth : defaults.depth),
    layoutDirection,
    direction,
    callerScope,
    edgeFilter: edgeFilter.length > 0 ? edgeFilter : defaults.edgeFilter,
    showUtilityCalls:
      typeof raw.showUtilityCalls === "boolean" ? raw.showUtilityCalls : defaults.showUtilityCalls,
    nodeFilter,
    showLegend: typeof raw.showLegend === "boolean" ? raw.showLegend : defaults.showLegend,
    showMiniMap: typeof raw.showMiniMap === "boolean" ? raw.showMiniMap : defaults.showMiniMap,
    bundleEdges: typeof raw.bundleEdges === "boolean" ? raw.bundleEdges : defaults.bundleEdges,
    groupingMode,
    maxNodes: clampMaxNodes(typeof raw.maxNodes === "number" ? raw.maxNodes : defaults.maxNodes),
  };
}

export function toTrailConfigDto(rootId: string, config: TrailUiConfig): TrailConfigDto {
  return {
    root_id: rootId,
    mode: config.mode,
    target_id: config.mode === "ToTargetSymbol" ? config.targetId : null,
    depth: clampDepth(config.depth),
    layout_direction: config.layoutDirection,
    direction: config.direction,
    caller_scope: config.callerScope,
    edge_filter: config.edgeFilter,
    show_utility_calls: config.showUtilityCalls,
    node_filter: config.nodeFilter,
    max_nodes: clampMaxNodes(config.maxNodes),
  };
}
