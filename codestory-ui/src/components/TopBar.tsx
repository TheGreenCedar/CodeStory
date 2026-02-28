import type { ChangeEvent } from "react";
import { FolderOpen, RefreshCcw, ScanSearch } from "lucide-react";

import { Button } from "../ui/primitives";

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
        <Button
          variant="primary"
          onClick={onOpenProject}
          disabled={isBusy}
          leadingIcon={<FolderOpen size={16} strokeWidth={2.5} aria-hidden />}
        >
          Open Project
        </Button>
        <Button
          onClick={() => onIndex("Incremental")}
          disabled={isBusy || !projectOpen}
          leadingIcon={<RefreshCcw size={16} strokeWidth={2.5} aria-hidden />}
        >
          Incremental Index
        </Button>
        <Button
          onClick={() => onIndex("Full")}
          disabled={isBusy || !projectOpen}
          leadingIcon={<ScanSearch size={16} strokeWidth={2.5} aria-hidden />}
        >
          Full Index
        </Button>
      </div>
    </header>
  );
}
