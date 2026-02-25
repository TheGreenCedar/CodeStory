import type { PersistedTrailUiConfig } from "../graph/trailConfig";

export type LeftTab = "agent" | "explorer";

export type PersistedLayout = {
  activeGraphId: string | null;
  expandedNodes: Record<string, boolean>;
  selectedTab: LeftTab;
  trailConfig?: PersistedTrailUiConfig;
};

export type PendingSymbolFocus = {
  symbolId: string;
  label: string;
};
