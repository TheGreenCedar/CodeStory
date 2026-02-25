import type { PendingSymbolFocus } from "../app/types";

type PendingFocusDialogProps = {
  pendingFocus: PendingSymbolFocus | null;
  onResolve: (decision: "save" | "discard" | "cancel") => Promise<void>;
};

export function PendingFocusDialog({ pendingFocus, onResolve }: PendingFocusDialogProps) {
  if (!pendingFocus) {
    return null;
  }

  return (
    <div className="confirm-overlay" role="presentation">
      <div className="confirm-modal" role="dialog" aria-modal="true" aria-label="Unsaved changes">
        <h3>Unsaved changes</h3>
        <p>Save your current edits before switching symbols?</p>
        <div className="confirm-actions">
          <button onClick={() => void onResolve("discard")}>Discard</button>
          <button onClick={() => void onResolve("cancel")}>Cancel</button>
          <button className="confirm-primary" onClick={() => void onResolve("save")}>
            Save &amp; Switch
          </button>
        </div>
      </div>
    </div>
  );
}
