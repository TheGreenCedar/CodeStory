export type LeftTab = "agent" | "explorer";

export type PersistedLayout = {
  activeGraphId: string | null;
  expandedNodes: Record<string, boolean>;
  selectedTab: LeftTab;
};

export type PendingSymbolFocus = {
  symbolId: string;
  label: string;
};
