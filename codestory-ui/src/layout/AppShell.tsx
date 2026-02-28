import type { ReactNode } from "react";

export const APP_SHELL_SECTIONS = [
  {
    id: "investigate",
    label: "Investigate",
    blurb: "Focused code investigation",
  },
  {
    id: "library",
    label: "Library",
    blurb: "Saved investigation spaces",
  },
  {
    id: "settings",
    label: "Settings",
    blurb: "Global preferences",
  },
] as const;

export type AppShellSection = (typeof APP_SHELL_SECTIONS)[number]["id"];

type AppShellProps = {
  activeSection: AppShellSection;
  onSelectSection: (section: AppShellSection) => void;
  workspace: ReactNode;
  sectionContent?: Partial<Record<AppShellSection, ReactNode>>;
};

export function AppShell({
  activeSection,
  onSelectSection,
  workspace,
  sectionContent,
}: AppShellProps) {
  const customContent = sectionContent?.[activeSection] ?? null;
  const activeLabel = APP_SHELL_SECTIONS.find((section) => section.id === activeSection)?.label;
  return (
    <div className="app-body">
      <aside className="app-nav" aria-label="Primary sections">
        <h2>CodeStory</h2>
        <p>One focused step at a time.</p>
        <div className="app-nav-links">
          {APP_SHELL_SECTIONS.map((section) => {
            const isActive = section.id === activeSection;
            return (
              <button
                key={section.id}
                type="button"
                className={isActive ? "app-nav-link app-nav-link-active" : "app-nav-link"}
                aria-current={isActive ? "page" : undefined}
                onClick={() => onSelectSection(section.id)}
              >
                <span>{section.label}</span>
                <small>{section.blurb}</small>
              </button>
            );
          })}
        </div>
      </aside>
      <section className="app-content" aria-label="Active section">
        {activeSection === "investigate" ? (
          workspace
        ) : customContent ? (
          customContent
        ) : (
          <div className="app-placeholder-card">
            <h3>{activeLabel}</h3>
            <p>This section is not available right now. Return to Investigate to continue.</p>
            <button type="button" onClick={() => onSelectSection("investigate")}>
              Open Investigate
            </button>
          </div>
        )}
      </section>
    </div>
  );
}
