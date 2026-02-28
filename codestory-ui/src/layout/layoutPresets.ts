export type InvestigateFocusMode = "ask" | "graph" | "code";

export const INVESTIGATE_FOCUS_MODE_KEY = "codestory:investigate-focus-mode:v1";
export const LEGACY_WORKSPACE_LAYOUT_PRESET_KEY = "codestory:workspace-layout-preset";

export const INVESTIGATE_FOCUS_MODES: InvestigateFocusMode[] = ["ask", "graph", "code"];

export function normalizeInvestigateFocusMode(raw: unknown): InvestigateFocusMode {
  if (raw === "ask" || raw === "graph" || raw === "code") {
    return raw;
  }
  if (raw === "learn") {
    return "graph";
  }
  if (raw === "debug") {
    return "graph";
  }
  if (raw === "review") {
    return "code";
  }
  return "graph";
}

export function investigateFocusModeLabel(mode: InvestigateFocusMode): string {
  if (mode === "ask") {
    return "Ask";
  }
  if (mode === "graph") {
    return "Graph";
  }
  return "Code";
}

export function migrateLegacyWorkspacePreset(raw: unknown): InvestigateFocusMode | null {
  if (raw === "learn") {
    return "graph";
  }
  if (raw === "debug") {
    return "graph";
  }
  if (raw === "review") {
    return "code";
  }
  return null;
}
