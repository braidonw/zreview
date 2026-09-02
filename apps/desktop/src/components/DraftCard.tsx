import "./DraftCard.css";

/** A resting draft shown under its row while the composer is closed; `isStale` is kept for parity though an anchored draft is never stale. */
export function DraftCard({ body, isStale = false }: { body: string; isStale?: boolean }) {
  return (
    <div className="draft-card">
      <div className="draft-card__header">
        <span className="draft-card__label">Your draft</span>
        {isStale && <span className="draft-card__stale">needs re-anchoring</span>}
      </div>
      <div className="draft-card__body">{body}</div>
    </div>
  );
}
