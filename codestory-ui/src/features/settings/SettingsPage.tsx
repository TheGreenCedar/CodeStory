import type { FeatureFlagState } from "../../app/featureFlags";

type SettingsPageProps = {
  featureFlags: FeatureFlagState;
  onUpdateFlag: (flag: keyof FeatureFlagState, value: boolean) => void;
};

export function SettingsPage({ featureFlags, onUpdateFlag }: SettingsPageProps) {
  return (
    <section className="settings-page" aria-label="Settings">
      <header className="settings-page-header">
        <h3>Settings</h3>
        <p>Global preferences only. Workflow controls stay in contextual drawers.</p>
      </header>

      <div className="settings-groups">
        <label className="settings-toggle">
          <input
            type="checkbox"
            checked={featureFlags.uxResetV2}
            onChange={(event) => onUpdateFlag("uxResetV2", event.target.checked)}
          />
          UX reset experience
        </label>

        <label className="settings-toggle">
          <input
            type="checkbox"
            checked={featureFlags.onboardingStarter}
            onChange={(event) => onUpdateFlag("onboardingStarter", event.target.checked)}
          />
          Show starter card
        </label>

        <label className="settings-toggle">
          <input
            type="checkbox"
            checked={featureFlags.singlePaneInvestigate}
            onChange={(event) => onUpdateFlag("singlePaneInvestigate", event.target.checked)}
          />
          Single-pane investigate mode
        </label>

        <label className="settings-toggle">
          <input
            type="checkbox"
            checked={featureFlags.spacesLibrary}
            onChange={(event) => onUpdateFlag("spacesLibrary", event.target.checked)}
          />
          Spaces library
        </label>
      </div>

      <section className="settings-help-card" aria-label="Keyboard shortcuts">
        <h4>Keyboard</h4>
        <p>`Ctrl+K` opens command palette for fast actions.</p>
      </section>
    </section>
  );
}
