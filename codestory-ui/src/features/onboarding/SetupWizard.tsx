import { useEffect, useId, useMemo, useState, type ChangeEvent, type ReactNode } from "react";

import type { IndexMode } from "../../generated/api";

export type SetupWizardIndexProgress = {
  current: number;
  total: number;
} | null;

export type SetupWizardProps = {
  projectPath: string;
  onProjectPathChange: (path: string) => void;
  projectOpen: boolean;
  indexProgress: SetupWizardIndexProgress;
  onOpenProject: () => void | Promise<void>;
  onIndex: (mode: IndexMode) => void | Promise<void>;
  onClose: () => void;
  defaultIndexMode?: IndexMode;
  isReady?: boolean;
  messageSlot?: ReactNode;
  className?: string;
};

const RECOMMENDED_INDEX_MODE: IndexMode = "Incremental";

function toProgressPercent(progress: Exclude<SetupWizardIndexProgress, null>): number {
  const max = Math.max(progress.total, 1);
  const normalized = Math.min(Math.max(progress.current, 0), max);
  return Math.round((normalized / max) * 100);
}

export function SetupWizard({
  projectPath,
  onProjectPathChange,
  projectOpen,
  indexProgress,
  onOpenProject,
  onIndex,
  onClose,
  defaultIndexMode = RECOMMENDED_INDEX_MODE,
  isReady,
  messageSlot,
  className,
}: SetupWizardProps) {
  const [selectedMode, setSelectedMode] = useState<IndexMode>(defaultIndexMode);
  const [hasRequestedIndex, setHasRequestedIndex] = useState<boolean>(indexProgress !== null);
  const pathInputId = useId();
  const modeGroupName = useId();

  useEffect(() => {
    setSelectedMode(defaultIndexMode);
  }, [defaultIndexMode]);

  useEffect(() => {
    if (indexProgress !== null) {
      setHasRequestedIndex(true);
    }
  }, [indexProgress]);

  const inferredReady = projectOpen && hasRequestedIndex && indexProgress === null;
  const ready = isReady ?? inferredReady;
  const activeStep = useMemo(() => {
    if (!projectOpen) {
      return 1;
    }
    if (!ready) {
      return 2;
    }
    return 3;
  }, [projectOpen, ready]);

  const pathMissing = projectPath.trim().length === 0;
  const modeHelpText =
    selectedMode === "Incremental"
      ? "Recommended for everyday work. Incremental indexing updates only what changed so you can start asking questions quickly."
      : "Use Full indexing after major branch switches or when you need a complete rebuild of symbol relationships.";

  const progressPercent = indexProgress ? toProgressPercent(indexProgress) : null;

  const handlePathChange = (event: ChangeEvent<HTMLInputElement>) => {
    onProjectPathChange(event.target.value);
  };

  const handleModeChange = (event: ChangeEvent<HTMLInputElement>) => {
    const nextMode: IndexMode = event.target.value === "Full" ? "Full" : "Incremental";
    setSelectedMode(nextMode);
  };

  const handleIndex = () => {
    setHasRequestedIndex(true);
    void onIndex(selectedMode);
  };

  return (
    <section className={className} aria-labelledby="setup-wizard-title">
      <header>
        <h2 id="setup-wizard-title">Setup wizard</h2>
        <p>Complete these three steps to get the project ready for exploration.</p>
        <button type="button" aria-label="Close setup wizard" onClick={onClose}>
          Close
        </button>
      </header>

      <ol aria-label="Setup progress">
        <li aria-current={activeStep === 1 ? "step" : undefined}>
          <strong>1. Confirm project path</strong>
          <p>{projectOpen ? "Done" : "Current"}</p>
        </li>
        <li aria-current={activeStep === 2 ? "step" : undefined}>
          <strong>2. Choose index mode</strong>
          <p>{ready ? "Done" : activeStep === 2 ? "Current" : "Upcoming"}</p>
        </li>
        <li aria-current={activeStep === 3 ? "step" : undefined}>
          <strong>3. Check readiness</strong>
          <p>{ready ? "Current" : "Upcoming"}</p>
        </li>
      </ol>

      <section aria-labelledby="setup-path-title">
        <h3 id="setup-path-title">Project path confirmation</h3>
        <label htmlFor={pathInputId}>Project path</label>
        <input
          id={pathInputId}
          type="text"
          value={projectPath}
          onChange={handlePathChange}
          placeholder="C:\\path\\to\\project"
          aria-invalid={pathMissing}
        />
        <button type="button" onClick={() => void onOpenProject()} disabled={pathMissing}>
          Open project
        </button>
        {pathMissing ? (
          <p role="status" aria-live="polite">
            Enter a project path to continue.
          </p>
        ) : null}
        <p role="status" aria-live="polite">
          {projectOpen ? "Project connection is active." : "Project is not open yet."}
        </p>
      </section>

      <section aria-labelledby="setup-index-title">
        <h3 id="setup-index-title">Index mode recommendation</h3>
        <fieldset>
          <legend>Index mode</legend>
          <label>
            <input
              type="radio"
              name={modeGroupName}
              value="Incremental"
              checked={selectedMode === "Incremental"}
              onChange={handleModeChange}
            />
            Incremental (recommended)
          </label>
          <p>Best default for daily development work.</p>
          <label>
            <input
              type="radio"
              name={modeGroupName}
              value="Full"
              checked={selectedMode === "Full"}
              onChange={handleModeChange}
            />
            Full index
          </label>
          <p>Use when your branch or dependencies changed significantly.</p>
        </fieldset>
        <p>{modeHelpText}</p>
        <button type="button" onClick={handleIndex} disabled={!projectOpen}>
          Start {selectedMode} index
        </button>
        {!projectOpen ? <p>Open the project first before starting an index run.</p> : null}
        {indexProgress ? (
          <div role="status" aria-live="polite">
            <p>
              Indexing {indexProgress.current} of {indexProgress.total} files.
            </p>
            <progress value={indexProgress.current} max={Math.max(indexProgress.total, 1)} />
            <p>{progressPercent}% complete</p>
          </div>
        ) : hasRequestedIndex ? (
          <p role="status" aria-live="polite">
            Index request sent. Continue when indexing is complete.
          </p>
        ) : null}
      </section>

      <section aria-labelledby="setup-ready-title">
        <h3 id="setup-ready-title">Readiness</h3>
        <ul>
          <li>Project open: {projectOpen ? "Ready" : "Pending"}</li>
          <li>Index complete: {ready ? "Ready" : "Pending"}</li>
        </ul>
        <p role="status" aria-live="polite">
          {ready
            ? "Ready to continue. You can ask your first question now."
            : "Complete the first two steps to finish setup."}
        </p>
        <button type="button" onClick={onClose} disabled={!ready}>
          Finish setup
        </button>
      </section>

      {messageSlot ? (
        <div aria-live="polite" aria-atomic="true">
          {messageSlot}
        </div>
      ) : null}
    </section>
  );
}
