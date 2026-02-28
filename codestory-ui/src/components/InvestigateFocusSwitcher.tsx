import {
  INVESTIGATE_FOCUS_MODES,
  investigateFocusModeLabel,
  type InvestigateFocusMode,
} from "../layout/layoutPresets";
import { Bolt, Code2, Orbit } from "lucide-react";
import type { ReactNode } from "react";

import { Button } from "../ui/primitives";

type InvestigateFocusSwitcherProps = {
  mode: InvestigateFocusMode;
  onModeChange: (mode: InvestigateFocusMode) => void;
};

export function InvestigateFocusSwitcher({ mode, onModeChange }: InvestigateFocusSwitcherProps) {
  const modeIcons: Record<InvestigateFocusMode, ReactNode> = {
    ask: <Bolt size={14} strokeWidth={2.5} aria-hidden />,
    graph: <Orbit size={14} strokeWidth={2.5} aria-hidden />,
    code: <Code2 size={14} strokeWidth={2.5} aria-hidden />,
  };
  return (
    <div className="focus-switcher" role="tablist" aria-label="Investigate focus mode">
      {INVESTIGATE_FOCUS_MODES.map((candidate) => {
        const isActive = candidate === mode;
        return (
          <Button
            key={candidate}
            role="tab"
            aria-selected={isActive}
            variant={isActive ? "primary" : "ghost"}
            className={
              isActive ? "focus-switcher-item focus-switcher-item-active" : "focus-switcher-item"
            }
            onClick={() => onModeChange(candidate)}
          >
            {modeIcons[candidate]}
            {investigateFocusModeLabel(candidate)}
          </Button>
        );
      })}
    </div>
  );
}
