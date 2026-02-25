import type { ChangeEvent } from "react";

type IndexMode = "Full" | "Incremental";

type TopBarProps = {
  isBusy: boolean;
  projectOpen: boolean;
  projectPath: string;
  onProjectPathChange: (path: string) => void;
  onOpenProject: () => void;
  onIndex: (mode: IndexMode) => void;
};

export function TopBar({
  isBusy,
  projectOpen,
  projectPath,
  onProjectPathChange,
  onOpenProject,
  onIndex,
}: TopBarProps) {
  const handlePathChange = (event: ChangeEvent<HTMLInputElement>) => {
    onProjectPathChange(event.target.value);
  };

  return (
    <header className="topbar">
      <div className="brand">
        <h1>CodeStory</h1>
        <p>Agentic code understanding with graph-grounded responses.</p>
      </div>
      <div className="topbar-actions">
        <input
          className="path-input"
          value={projectPath}
          onChange={handlePathChange}
          placeholder="Project path"
          aria-label="Project path"
        />
        <button onClick={onOpenProject} disabled={isBusy}>
          Open Project
        </button>
        <button onClick={() => onIndex("Incremental")} disabled={isBusy || !projectOpen}>
          Incremental Index
        </button>
        <button onClick={() => onIndex("Full")} disabled={isBusy || !projectOpen}>
          Full Index
        </button>
      </div>
    </header>
  );
}
