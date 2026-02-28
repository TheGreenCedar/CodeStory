import { useEffect, useMemo, useRef, useState } from "react";

export type CommandPaletteCommand = {
  id: string;
  label: string;
  detail?: string;
  keywords?: string[];
  disabled?: boolean;
  run: () => void | Promise<void>;
};

function commandScore(command: CommandPaletteCommand, query: string): number {
  if (query.length === 0) {
    return 1;
  }

  const normalizedLabel = command.label.toLowerCase();
  const normalizedId = command.id.toLowerCase();
  const normalizedKeywords = (command.keywords ?? []).join(" ").toLowerCase();
  const haystack = `${normalizedLabel} ${normalizedId} ${normalizedKeywords}`;
  const terms = query.split(/\s+/).filter((term) => term.length > 0);

  if (terms.length === 0) {
    return 1;
  }

  if (terms.some((term) => !haystack.includes(term))) {
    return -1;
  }

  let score = 0;
  for (const term of terms) {
    if (normalizedLabel.startsWith(term)) {
      score += 120;
      continue;
    }
    if (normalizedLabel.includes(term)) {
      score += 90;
      continue;
    }
    if (normalizedKeywords.includes(term)) {
      score += 60;
      continue;
    }
    score += 30;
  }

  return score;
}

export function rankCommands(
  commands: CommandPaletteCommand[],
  query: string,
): CommandPaletteCommand[] {
  const normalizedQuery = query.trim().toLowerCase();
  return commands
    .map((command, index) => ({
      command,
      index,
      score: commandScore(command, normalizedQuery),
    }))
    .filter((entry) => entry.score >= 0)
    .sort((left, right) => right.score - left.score || left.index - right.index)
    .map((entry) => entry.command);
}

type CommandPaletteProps = {
  open: boolean;
  commands: CommandPaletteCommand[];
  onClose: () => void;
};

export function CommandPalette({ open, commands, onClose }: CommandPaletteProps) {
  const [query, setQuery] = useState<string>("");
  const [activeIndex, setActiveIndex] = useState<number>(0);
  const inputRef = useRef<HTMLInputElement | null>(null);
  const rankedCommands = useMemo(() => rankCommands(commands, query), [commands, query]);

  useEffect(() => {
    if (!open) {
      return;
    }
    setQuery("");
    setActiveIndex(0);
    window.setTimeout(() => inputRef.current?.focus(), 0);
  }, [open]);

  useEffect(() => {
    setActiveIndex((previous) => {
      if (rankedCommands.length === 0) {
        return 0;
      }
      return Math.min(previous, rankedCommands.length - 1);
    });
  }, [rankedCommands.length]);

  useEffect(() => {
    if (!open) {
      return;
    }

    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        onClose();
        return;
      }

      if (event.key === "ArrowDown") {
        event.preventDefault();
        setActiveIndex((previous) =>
          rankedCommands.length === 0 ? 0 : (previous + 1) % rankedCommands.length,
        );
        return;
      }

      if (event.key === "ArrowUp") {
        event.preventDefault();
        setActiveIndex((previous) => {
          if (rankedCommands.length === 0) {
            return 0;
          }
          return (previous - 1 + rankedCommands.length) % rankedCommands.length;
        });
        return;
      }

      if (event.key === "Enter") {
        event.preventDefault();
        const activeCommand = rankedCommands[activeIndex];
        if (!activeCommand || activeCommand.disabled) {
          return;
        }
        void activeCommand.run();
        onClose();
      }
    };

    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [activeIndex, onClose, open, rankedCommands]);

  if (!open) {
    return null;
  }

  return (
    <div
      className="command-palette-overlay"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) {
          onClose();
        }
      }}
    >
      <div className="command-palette" role="dialog" aria-modal="true" aria-label="Command palette">
        <label htmlFor="command-palette-search">Search commands</label>
        <input
          id="command-palette-search"
          ref={inputRef}
          value={query}
          onChange={(event) => setQuery(event.target.value)}
          placeholder="Type a command (for example: open, index, trail)"
        />
        <div className="command-palette-results" role="listbox" aria-label="Command results">
          {rankedCommands.length > 0 ? (
            rankedCommands.map((command, index) => {
              const isActive = index === activeIndex;
              return (
                <button
                  key={command.id}
                  type="button"
                  role="option"
                  disabled={command.disabled}
                  aria-selected={isActive}
                  className={
                    isActive
                      ? "command-palette-item command-palette-item-active"
                      : "command-palette-item"
                  }
                  onMouseEnter={() => setActiveIndex(index)}
                  onClick={() => {
                    if (command.disabled) {
                      return;
                    }
                    void command.run();
                    onClose();
                  }}
                >
                  <span>{command.label}</span>
                  {command.detail ? <small>{command.detail}</small> : null}
                </button>
              );
            })
          ) : (
            <p className="command-palette-empty">No commands match this query.</p>
          )}
        </div>
      </div>
    </div>
  );
}
