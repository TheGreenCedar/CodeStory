import type { AgentConnectionSettingsDto } from "../generated/api";
import type { PersistedTrailUiConfig } from "../graph/trailConfig";

export type LeftTab = "agent" | "explorer";

export type PersistedLayout = {
  activeGraphId: string | null;
  expandedNodes: Record<string, boolean>;
  selectedTab: LeftTab;
  trailConfig?: PersistedTrailUiConfig;
  agentConnection?: AgentConnectionSettingsDto;
};

export type PendingSymbolFocus = {
  symbolId: string;
  label: string;
  graphMode?: "neighborhood" | "trailDepthOne";
};
