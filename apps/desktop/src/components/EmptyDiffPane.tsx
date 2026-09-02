import "./EmptyDiffPane.css";

/**
 * Shown instead of the row list when a file has nothing to display, so an
 * empty pane reads as an explanation rather than a bug.
 */
export function EmptyDiffPane({ label, detail }: { label: string; detail: string }) {
  return (
    <div className="empty-diff-pane">
      <div className="empty-diff-pane__label">{label}</div>
      <div className="empty-diff-pane__detail">{detail}</div>
    </div>
  );
}
