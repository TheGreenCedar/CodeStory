import { useEffect, useMemo, useState } from "react";

type StarterCardProps = {
  projectPath: string;
  projectOpen: boolean;
  indexComplete: boolean;
  askedFirstQuestion: boolean;
  inspectedSource: boolean;
  onOpenProject: () => void | Promise<void>;
  onRunIndex: () => void | Promise<void>;
  onSeedQuestion: () => void | Promise<void>;
  onInspectSource: () => void | Promise<void>;
  onPrimaryAction?: (action: string) => void;
  className?: string;
};

const STARTER_STORAGE_NS = "codestory:onboarding:starter:v1";

function storageKey(projectPath: string): string {
  const normalized = projectPath.trim().length > 0 ? projectPath.trim() : ".";
  return `${STARTER_STORAGE_NS}:${encodeURIComponent(normalized)}`;
}

function readDismissedState(key: string): boolean {
  if (typeof window === "undefined") {
    return false;
  }
  return window.localStorage.getItem(key) === "dismissed";
}

function writeDismissedState(key: string, dismissed: boolean): void {
  if (typeof window === "undefined") {
    return;
  }
  if (!dismissed) {
    window.localStorage.removeItem(key);
    return;
  }
  window.localStorage.setItem(key, "dismissed");
}

export function StarterCard({
  projectPath,
  projectOpen,
  indexComplete,
  askedFirstQuestion,
  inspectedSource,
  onOpenProject,
  onRunIndex,
  onSeedQuestion,
  onInspectSource,
  onPrimaryAction,
  className,
}: StarterCardProps) {
  const key = useMemo(() => storageKey(projectPath), [projectPath]);
  const [helpOpen, setHelpOpen] = useState<boolean>(false);
  const [dismissed, setDismissed] = useState<boolean>(() => readDismissedState(key));

  useEffect(() => {
    setDismissed(readDismissedState(key));
  }, [key]);

  const nextStep = useMemo(() => {
    if (!projectOpen) {
      return {
        action: "open_project",
        title: "Open project",
        detail: "Connect your repository to start exploring.",
        run: onOpenProject,
      };
    }
    if (!indexComplete) {
      return {
        action: "start_index",
        title: "Start index",
        detail: "Build symbol context so answers and graph jumps work.",
        run: onRunIndex,
      };
    }
    if (!askedFirstQuestion) {
      return {
        action: "ask_first_question",
        title: "Ask first question",
        detail: "Generate a first architecture answer.",
        run: onSeedQuestion,
      };
    }
    if (!inspectedSource) {
      return {
        action: "inspect_source",
        title: "Inspect source",
        detail: "Jump into code to verify what you learned.",
        run: onInspectSource,
      };
    }
    return {
      action: "complete",
      title: "Ready",
      detail: "You are set for focused investigations.",
      run: null,
    };
  }, [
    askedFirstQuestion,
    indexComplete,
    inspectedSource,
    onInspectSource,
    onOpenProject,
    onRunIndex,
    onSeedQuestion,
    projectOpen,
  ]);

  if (dismissed) {
    return (
      <section className={className}>
        <p>Starter card dismissed.</p>
        <button
          type="button"
          onClick={() => {
            setDismissed(false);
            writeDismissedState(key, false);
          }}
        >
          Show starter
        </button>
      </section>
    );
  }

  return (
    <section className={className} aria-label="First-run starter">
      <div className="starter-header">
        <h2>Start Here</h2>
        <button
          type="button"
          className="starter-dismiss"
          onClick={() => {
            setDismissed(true);
            writeDismissedState(key, true);
          }}
        >
          Dismiss
        </button>
      </div>

      <div className="starter-status-chips" aria-label="Readiness status">
        <span className={projectOpen ? "starter-chip starter-chip-ready" : "starter-chip"}>
          Project
        </span>
        <span className={indexComplete ? "starter-chip starter-chip-ready" : "starter-chip"}>
          Index
        </span>
        <span
          className={
            projectOpen && indexComplete && askedFirstQuestion && inspectedSource
              ? "starter-chip starter-chip-ready"
              : "starter-chip"
          }
        >
          Ready
        </span>
      </div>

      <div className="starter-next-step">
        <strong>{nextStep.title}</strong>
        <p>{nextStep.detail}</p>
      </div>

      {nextStep.run ? (
        <button
          type="button"
          className="starter-primary"
          onClick={() => {
            onPrimaryAction?.(nextStep.action);
            void nextStep.run();
          }}
        >
          {nextStep.title}
        </button>
      ) : null}

      <button
        type="button"
        className="starter-help-toggle"
        onClick={() => setHelpOpen((prev) => !prev)}
        aria-expanded={helpOpen}
      >
        Need help?
      </button>

      {helpOpen ? (
        <div className="starter-help">
          <p>Use `Ctrl+K` for quick actions and switch focus modes to keep context clear.</p>
        </div>
      ) : null}
    </section>
  );
}
