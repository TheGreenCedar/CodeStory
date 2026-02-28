import { useEffect, useMemo, useState, type ReactNode } from "react";

export type ChecklistProps = {
  projectPath: string;
  projectOpen: boolean;
  indexComplete: boolean;
  askedFirstQuestion: boolean;
  inspectedSource: boolean;
  onOpenProject: () => void | Promise<void>;
  onIndex: () => void | Promise<void>;
  onAskFirstQuestion: () => void | Promise<void>;
  onInspectSource: () => void | Promise<void>;
  onVisibilityChange?: (visible: boolean) => void;
  className?: string;
  title?: ReactNode;
};

type ChecklistStorageState = {
  dismissed: boolean;
};

type ChecklistItem = {
  id: string;
  title: string;
  detail: string;
  complete: boolean;
  actionLabel: string;
  onAction: () => void | Promise<void>;
  disabled?: boolean;
};

export const CHECKLIST_STORAGE_NAMESPACE = "codestory:onboarding:checklist:v1";

function normalizeProjectPath(projectPath: string): string {
  const trimmed = projectPath.trim();
  return trimmed.length > 0 ? trimmed : ".";
}

export function checklistStorageKey(projectPath: string): string {
  return `${CHECKLIST_STORAGE_NAMESPACE}:${encodeURIComponent(normalizeProjectPath(projectPath))}`;
}

function readDismissedState(storageKey: string): boolean {
  if (typeof window === "undefined") {
    return false;
  }

  const raw = window.localStorage.getItem(storageKey);
  if (!raw) {
    return false;
  }

  try {
    const parsed = JSON.parse(raw) as ChecklistStorageState;
    return parsed.dismissed === true;
  } catch {
    return false;
  }
}

function persistDismissedState(storageKey: string, dismissed: boolean): void {
  if (typeof window === "undefined") {
    return;
  }

  if (!dismissed) {
    window.localStorage.removeItem(storageKey);
    return;
  }

  const payload: ChecklistStorageState = {
    dismissed: true,
  };
  window.localStorage.setItem(storageKey, JSON.stringify(payload));
}

export function Checklist({
  projectPath,
  projectOpen,
  indexComplete,
  askedFirstQuestion,
  inspectedSource,
  onOpenProject,
  onIndex,
  onAskFirstQuestion,
  onInspectSource,
  onVisibilityChange,
  className,
  title = "First-run checklist",
}: ChecklistProps) {
  const storageKey = useMemo(() => checklistStorageKey(projectPath), [projectPath]);
  const [dismissed, setDismissed] = useState<boolean>(() => readDismissedState(storageKey));

  useEffect(() => {
    setDismissed(readDismissedState(storageKey));
  }, [storageKey]);

  const items = useMemo<ChecklistItem[]>(
    () => [
      {
        id: "open-project",
        title: "Open project",
        detail: "Confirm the target project path and open it.",
        complete: projectOpen,
        actionLabel: "Open project",
        onAction: onOpenProject,
      },
      {
        id: "index-complete",
        title: "Index complete",
        detail: "Run indexing so answers and graph lookups can reference symbols.",
        complete: indexComplete,
        actionLabel: "Run index",
        onAction: onIndex,
        disabled: !projectOpen,
      },
      {
        id: "ask-first-question",
        title: "Ask first question",
        detail: "Ask the assistant to explain a flow in your codebase.",
        complete: askedFirstQuestion,
        actionLabel: "Ask first question",
        onAction: onAskFirstQuestion,
        disabled: !indexComplete,
      },
      {
        id: "inspect-source",
        title: "Inspect source",
        detail: "Open a symbol and inspect source context from the code pane.",
        complete: inspectedSource,
        actionLabel: "Inspect source",
        onAction: onInspectSource,
        disabled: !projectOpen,
      },
    ],
    [
      askedFirstQuestion,
      indexComplete,
      inspectedSource,
      onAskFirstQuestion,
      onIndex,
      onInspectSource,
      onOpenProject,
      projectOpen,
    ],
  );

  const completedCount = items.filter((item) => item.complete).length;
  const allComplete = completedCount === items.length;

  const setVisibility = (visible: boolean) => {
    const nextDismissed = !visible;
    setDismissed(nextDismissed);
    persistDismissedState(storageKey, nextDismissed);
    onVisibilityChange?.(visible);
  };

  if (dismissed) {
    return (
      <section className={className} aria-label="Onboarding checklist controls">
        <p>Checklist dismissed for this project.</p>
        <button type="button" onClick={() => setVisibility(true)}>
          Reopen checklist
        </button>
      </section>
    );
  }

  return (
    <section className={className} aria-labelledby="onboarding-checklist-title">
      <header>
        <h2 id="onboarding-checklist-title">{title}</h2>
        <p>
          {completedCount} of {items.length} steps complete.
        </p>
        <button type="button" onClick={() => setVisibility(false)}>
          Dismiss checklist
        </button>
      </header>

      <ul>
        {items.map((item) => (
          <li key={item.id}>
            <div>
              <h3>{item.title}</h3>
              <p>{item.detail}</p>
            </div>
            {item.complete ? (
              <span aria-label={`${item.title} complete`}>Done</span>
            ) : (
              <button type="button" onClick={() => void item.onAction()} disabled={item.disabled}>
                {item.actionLabel}
              </button>
            )}
          </li>
        ))}
      </ul>

      {allComplete ? (
        <p role="status" aria-live="polite">
          All onboarding steps are complete.
        </p>
      ) : null}
    </section>
  );
}
