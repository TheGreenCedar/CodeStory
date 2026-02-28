import { Keyboard, Paintbrush2, Radar } from "lucide-react";

import { Badge, Card } from "../../ui/primitives";

export function SettingsPage() {
  return (
    <section className="settings-page" aria-label="Settings">
      <header className="settings-page-header">
        <h3>
          <Paintbrush2 size={16} strokeWidth={2.5} aria-hidden /> Settings
        </h3>
        <p>Playful Geometric theme is now the default experience for every workspace.</p>
      </header>

      <div className="settings-groups">
        <Card className="settings-toggle" as="article">
          <h4>Visual System</h4>
          <p>
            Design tokens, typography, motion, and graph chrome are centralized in `src/theme/`.
          </p>
          <Badge tone="accent">Active</Badge>
        </Card>

        <Card className="settings-toggle" as="article">
          <h4>Workspace Mode</h4>
          <p>
            Single-pane Investigate and Spaces Library are always enabled in the canonical shell.
          </p>
          <Badge tone="quaternary">Canonical</Badge>
        </Card>

        <Card className="settings-toggle" as="article">
          <h4>Accessibility</h4>
          <p>
            Focus rings, high-contrast states, large tap targets, and reduced-motion fallbacks are
            enforced by default.
          </p>
          <Badge tone="secondary">WCAG-minded</Badge>
        </Card>
      </div>

      <Card className="settings-help-card" as="section" aria-label="Keyboard shortcuts">
        <h4>
          <Keyboard size={16} strokeWidth={2.5} aria-hidden /> Keyboard
        </h4>
        <p>`Ctrl+K` opens command palette for fast actions.</p>
        <p>`Ctrl+U` opens trail settings while focused in graph view.</p>
      </Card>

      <Card className="settings-help-card" as="section" aria-label="Graph semantics">
        <h4>
          <Radar size={16} strokeWidth={2.5} aria-hidden /> Graph Semantics
        </h4>
        <p>Edge colors remain semantically distinct while adopting playful tokens.</p>
      </Card>
    </section>
  );
}
