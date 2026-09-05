import type { SubmissionPhaseDto, SubmissionRequestDto } from "../bindings";
import "./SubmissionPanel.css";

/**
 * Everything about the submission that belongs above the diff.
 *
 * A full panel rather than a small dialog, because the reviewer has to see every
 * inline comment, the summary, the verdict, and the head it is pinned to before
 * anything is posted.
 */
export function SubmissionPanel({
  phase,
  onCancel,
  onSend,
}: {
  phase: SubmissionPhaseDto;
  onCancel: () => void;
  onSend: () => void;
}) {
  switch (phase.state) {
    case "Idle":
      return null;

    case "Sending":
      return (
        <section className="submission-panel">
          <p className="submission-panel__sending">Submitting the review...</p>
        </section>
      );

    case "Sent":
      return (
        <section className="submission-panel">
          <p className="submission-panel__sent">{phase.outcome.heading}</p>
          <p className="submission-panel__url">{phase.outcome.url}</p>
        </section>
      );

    case "Failed":
      return (
        <section className="submission-panel">
          <p className="submission-panel__failed">{phase.failure.summary}</p>
          {phase.failure.remediation !== null && (
            <p className="submission-panel__remediation">{phase.failure.remediation}</p>
          )}
          {phase.failure.detail !== null && (
            <p className="submission-panel__detail">{phase.failure.detail}</p>
          )}
        </section>
      );

    case "Confirming":
      return <Confirmation request={phase.request} onCancel={onCancel} onSend={onSend} />;

    default: {
      // A submission state the backend grew that nothing here can draw. Drawing
      // nothing would hide a review that may be in flight or may have failed.
      const unknown: never = phase;
      throw new Error(`the submission is in an unknown state ${JSON.stringify(unknown)}`);
    }
  }
}

/** The full request, laid out for approval. Nothing has been posted yet. */
function Confirmation({
  request,
  onCancel,
  onSend,
}: {
  request: SubmissionRequestDto;
  onCancel: () => void;
  onSend: () => void;
}) {
  return (
    <section className="submission-panel">
      <p className="submission-panel__heading">{request.heading}</p>
      <p className="submission-panel__pinned">{request.pinned}</p>
      {request.body !== "" && <p className="submission-panel__body">{request.body}</p>}
      <ul className="submission-panel__comments">
        {request.comments.map((comment, index) => (
          <li key={`${comment.position}-${index}`} className="submission-panel__comment">
            <span className="submission-panel__position">{comment.position}</span>
            <span className="submission-panel__comment-body">{comment.body}</span>
          </li>
        ))}
      </ul>
      {request.excluded_heading !== null && (
        <div className="submission-panel__excluded">
          <p className="submission-panel__excluded-heading">{request.excluded_heading}</p>
          <ul className="submission-panel__excluded-list">
            {request.excluded.map((draft, index) => (
              <li key={`${draft.position}-${index}`} className="submission-panel__excluded-draft">
                {draft.position} ({draft.reason}): {draft.body}
              </li>
            ))}
          </ul>
        </div>
      )}
      <div className="submission-panel__actions">
        <button type="button" className="submission-panel__send" onClick={onSend}>
          Post this review to GitHub
        </button>
        <button type="button" className="submission-panel__cancel" onClick={onCancel}>
          Cancel
        </button>
      </div>
    </section>
  );
}
