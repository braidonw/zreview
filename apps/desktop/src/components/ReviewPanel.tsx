import type { FindingConflict } from "../hooks/sessionReducer";
import type { FindingDto, GuidanceDto, ReviewPanelDto, SeverityDto } from "../bindings";
import "./ReviewPanel.css";

const SEVERITY_LABEL: Record<SeverityDto, string> = {
  Error: "error",
  Warning: "warning",
  Info: "info",
};

/**
 * What this review is held to, the findings it proposes, and how far the run
 * that populates it has got.
 *
 * Two things this panel is careful to show rather than hide. What was refused, so
 * a run that kept one claim out of twelve does not read as a run that found one.
 * And what was never looked at, so a review that skipped files cannot present
 * itself as having covered the change.
 */
export function ReviewPanel({
  panel,
  notice,
  findingConflict,
  onRunReview,
  onCancelReview,
  onToggleGuidanceSection,
  onToggleGuidanceFile,
  onRevealFinding,
  onAcceptFinding,
  onDismissFinding,
  onReplaceFinding,
  onKeepFinding,
}: {
  panel: ReviewPanelDto;
  /** What a review command refused, until the next panel lands. */
  notice: string | null;
  /** The confirmation accepting a finding onto an occupied line is asking. */
  findingConflict: FindingConflict;
  onRunReview: () => void;
  onCancelReview: () => void;
  onToggleGuidanceSection: () => void;
  onToggleGuidanceFile: (path: string) => void;
  onRevealFinding: (id: number) => void;
  onAcceptFinding: (id: number) => void;
  onDismissFinding: (id: number) => void;
  onReplaceFinding: (id: number) => void;
  onKeepFinding: () => void;
}) {
  const { run, note, findings, footer } = panel;
  const isRunning = run.state === "Running";

  return (
    <aside className="review-panel">
      <header className="review-panel__header">
        <div className="review-panel__title">
          <span className="review-panel__heading">{panel.heading}</span>
          <button
            type="button"
            className={`review-panel__run review-panel__run--${isRunning ? "cancel" : "start"}`}
            onClick={isRunning ? onCancelReview : onRunReview}
          >
            {isRunning ? "Cancel" : "Review"}
          </button>
        </div>
        {run.state === "Running" && <p className="review-panel__progress">{run.detail}</p>}
      </header>
      {notice !== null && <p className="review-panel__notice">{notice}</p>}
      <GuidanceSection
        guidance={panel.guidance}
        onToggleSection={onToggleGuidanceSection}
        onToggleFile={onToggleGuidanceFile}
      />
      <div className="review-panel__body">
        {findings.length > 0 ? (
          <ul className="review-panel__findings">
            {findings.map((finding) => (
              <FindingCard
                key={finding.id}
                finding={finding}
                conflict={
                  findingConflict !== null && findingConflict.id === finding.id
                    ? findingConflict
                    : null
                }
                onReveal={() => onRevealFinding(finding.id)}
                onAccept={() => onAcceptFinding(finding.id)}
                onDismiss={() => onDismissFinding(finding.id)}
                onReplace={() => onReplaceFinding(finding.id)}
                onKeep={onKeepFinding}
              />
            ))}
          </ul>
        ) : (
          note !== null && (
            <div className="review-panel__note">
              <p className="review-panel__note-heading">{note.heading}</p>
              {note.detail !== null && <p className="review-panel__note-detail">{note.detail}</p>}
            </div>
          )
        )}
      </div>
      {footer !== null && (
        <footer className="review-panel__footer">
          {footer.refused !== null && <p>{footer.refused}</p>}
          {footer.not_reviewed !== null && (
            <>
              <p className="review-panel__not-reviewed">{footer.not_reviewed}</p>
              <ul className="review-panel__unreviewed">
                {footer.unreviewed.map((path) => (
                  <li key={path}>{path}</li>
                ))}
              </ul>
            </>
          )}
        </footer>
      )}
    </aside>
  );
}

/**
 * The guidance a run would be held to, and what of it will be sent.
 *
 * Open before the first run, because what leaves the machine has to be seen
 * before it is sent, and collapsed to its summary line once a run has happened.
 */
function GuidanceSection({
  guidance,
  onToggleSection,
  onToggleFile,
}: {
  guidance: GuidanceDto;
  onToggleSection: () => void;
  onToggleFile: (path: string) => void;
}) {
  if (guidance.kind === "NothingFound") {
    // Discovery ran and found nothing. Saying so is not the same as showing
    // nothing. A reviewer needs to tell "this repository states no conventions"
    // from "guidance was never looked for".
    return (
      <section className="review-panel__guidance">
        <p className="review-panel__nothing-found">{guidance.note}</p>
      </section>
    );
  }

  return (
    <section className="review-panel__guidance">
      <button
        type="button"
        className="review-panel__guidance-header"
        aria-expanded={guidance.expanded}
        onClick={onToggleSection}
      >
        <span>{guidance.summary}</span>
        <span className="review-panel__disclosure">{guidance.expanded ? "hide" : "show"}</span>
      </button>
      {guidance.expanded && (
        <>
          {guidance.entries.map((entry) => (
            <button
              key={entry.path}
              type="button"
              className="review-panel__entry"
              aria-pressed={entry.included}
              onClick={() => onToggleFile(entry.path)}
            >
              {/* A filled box is sent, an empty one is not. This is the disclosure control. */}
              <span
                className={`review-panel__include review-panel__include--${
                  entry.included ? "on" : "off"
                }`}
                aria-hidden="true"
              />
              <span
                className={`review-panel__entry-path review-panel__entry-path--${
                  entry.included ? "on" : "off"
                }`}
              >
                {entry.path}
              </span>
              <span className="review-panel__entry-meta">
                {entry.scope} · {entry.kilobytes}K
              </span>
            </button>
          ))}
          {/* Found and not used, each with its reason. Silently dropping a file
              the reviewer expected to matter is worse than not finding it. */}
          {guidance.skipped.map((skip) => (
            <div key={skip.path} className="review-panel__skipped">
              <span>{skip.path}</span>
              <span className="review-panel__skipped-reason">{skip.reason}</span>
            </div>
          ))}
          {guidance.excluded !== null && (
            <p className="review-panel__excluded">{guidance.excluded}</p>
          )}
        </>
      )}
    </section>
  );
}

/**
 * One proposal from a review backend. Nothing here is a comment until the
 * reviewer accepts it, which is why every card carries both what the model
 * decided and a plain Accept or Dismiss.
 */
function FindingCard({
  finding,
  conflict,
  onReveal,
  onAccept,
  onDismiss,
  onReplace,
  onKeep,
}: {
  finding: FindingDto;
  /** Present when accepting this finding found the anchor already occupied. */
  conflict: FindingConflict;
  onReveal: () => void;
  onAccept: () => void;
  onDismiss: () => void;
  onReplace: () => void;
  onKeep: () => void;
}) {
  return (
    <li
      className={`review-panel__finding ${
        finding.is_selected ? "review-panel__finding--selected" : ""
      }`}
      aria-selected={finding.is_selected}
      onClick={onReveal}
    >
      <div className="review-panel__finding-meta">
        <span
          className={`review-panel__severity review-panel__severity--${SEVERITY_LABEL[finding.severity]}`}
        >
          {SEVERITY_LABEL[finding.severity]}
        </span>
        <span className="review-panel__confidence">{finding.confidence_percent}%</span>
        <span className="review-panel__position">{finding.position ?? "whole change"}</span>
      </div>
      <p className="review-panel__finding-title">{finding.title}</p>
      {finding.rationale !== "" && (
        <p className="review-panel__finding-rationale">{finding.rationale}</p>
      )}
      {finding.citations.length > 0 && (
        <p className="review-panel__finding-citations">per {finding.citations.join(", ")}</p>
      )}
      <p className="review-panel__finding-origin">Proposed by {finding.origin}</p>
      {conflict !== null ? (
        <div className="review-panel__finding-conflict">
          <p className="review-panel__finding-conflict-intro">
            Replace your comment with this proposal?
          </p>
          <p className="review-panel__finding-conflict-text">
            <span className="review-panel__finding-conflict-label">Your comment</span>
            {conflict.existing}
          </p>
          <p className="review-panel__finding-conflict-text">
            <span className="review-panel__finding-conflict-label">Proposal</span>
            {conflict.proposed}
          </p>
          <div className="review-panel__finding-actions">
            <button
              type="button"
              className="review-panel__finding-replace"
              onClick={(event) => {
                event.stopPropagation();
                onReplace();
              }}
            >
              Replace
            </button>
            <button
              type="button"
              className="review-panel__finding-keep"
              onClick={(event) => {
                event.stopPropagation();
                onKeep();
              }}
            >
              Keep
            </button>
          </div>
        </div>
      ) : (
        <div className="review-panel__finding-actions">
          {/* No summary editor yet, so a whole-change finding can only be dismissed. */}
          {finding.position !== null && (
            <button
              type="button"
              className="review-panel__finding-accept"
              onClick={(event) => {
                event.stopPropagation();
                onAccept();
              }}
            >
              Accept
            </button>
          )}
          <button
            type="button"
            className="review-panel__finding-dismiss"
            onClick={(event) => {
              event.stopPropagation();
              onDismiss();
            }}
          >
            Dismiss
          </button>
        </div>
      )}
    </li>
  );
}
