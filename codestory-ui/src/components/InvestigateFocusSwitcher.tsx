import {
  INVESTIGATE_FOCUS_MODES,
  investigateFocusModeLabel,
  type InvestigateFocusMode,
} from "../layout/layoutPresets";

type InvestigateFocusSwitcherProps = {
  mode: InvestigateFocusMode;
  onModeChange: (mode: InvestigateFocusMode) => void;
};

export function InvestigateFocusSwitcher({ mode, onModeChange }: InvestigateFocusSwitcherProps) {
  return (
    <div className="focus-switcher" role="tablist" aria-label="Investigate focus mode">
      {INVESTIGATE_FOCUS_MODES.map((candidate) => {
        const isActive = candidate === mode;
        return (
          <button
            key={candidate}
            type="button"
            role="tab"
            aria-selected={isActive}
            className={
              isActive ? "focus-switcher-item focus-switcher-item-active" : "focus-switcher-item"
            }
            onClick={() => onModeChange(candidate)}
          >
            {investigateFocusModeLabel(candidate)}
          </button>
        );
      })}
    </div>
  );
}
