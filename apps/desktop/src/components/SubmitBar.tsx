import type { ReviewEventDto } from "../bindings";
import { SummaryEditor } from "./SummaryEditor";
import "./SubmitBar.css";

/** The three verdicts, in the order the GPUI bar offers them. */
const ACTIONS: { event: ReviewEventDto; label: string; tone: string }[] = [
  { event: "Comment", label: "Comment", tone: "comment" },
  { event: "Approve", label: "Approve", tone: "approve" },
  { event: "RequestChanges", label: "Request changes", tone: "changes" },
];

/**
 * What would be submitted, the summary that goes with it, and the three ways to
 * submit it.
 *
 * Choosing an action posts nothing. It opens the confirmation holding the exact
 * request, so what the reviewer approves is what leaves the machine.
 */
export function SubmitBar({
  readyCount,
  notAnchoredCount,
  summary,
  isSending,
  onSummaryChange,
  onSubmit,
}: {
  readyCount: number;
  notAnchoredCount: number;
  /** What the editor should hold, which moves only when the backend says so. */
  summary: string;
  /** True while a review is in flight, when no other submission may be started. */
  isSending: boolean;
  onSummaryChange: (body: string) => void;
  onSubmit: (event: ReviewEventDto) => void;
}) {
  return (
    <div className="submit-bar">
      <div className="submit-bar__counts">
        <span className="submit-bar__ready">{readyCount} to submit</span>
        {notAnchoredCount > 0 && (
          <span className="submit-bar__stale">{notAnchoredCount} not anchored</span>
        )}
      </div>
      <SummaryEditor summary={summary} onChange={onSummaryChange} />
      <div className="submit-bar__actions">
        {ACTIONS.map(({ event, label, tone }) => (
          <button
            key={event}
            type="button"
            className={`submit-bar__action submit-bar__action--${tone}`}
            disabled={isSending}
            onClick={() => onSubmit(event)}
          >
            {label}
          </button>
        ))}
      </div>
    </div>
  );
}
