import type { DiffSideDto, StaleDraftDto } from "../bindings";
import "./DraftsPanel.css";

/** The reviewer's drafts on the selected file and the ones that need re-anchoring, as one relocatable component. */
export function DraftsPanel({
  fileDraftCount,
  stale,
  cursor,
  onReanchor,
}: {
  fileDraftCount: number;
  stale: StaleDraftDto[];
  cursor: number;
  onReanchor: (path: string, side: DiffSideDto, line: number, row: number) => void;
}) {
  if (fileDraftCount === 0) {
    return null;
  }

  return (
    <div className="drafts-panel">
      <div className="drafts-panel__count">{fileDraftCount} of your drafts</div>
      {stale.length > 0 && (
        <>
          <div className="drafts-panel__stale-heading">
            {stale.length} draft{stale.length === 1 ? "" : "s"} need re-anchoring
          </div>
          {stale.map((draft) => (
            <div key={`${draft.path}:${draft.side}:${draft.line}`} className="drafts-panel__card">
              <div className="drafts-panel__location">{draft.location}</div>
              <div className="drafts-panel__body">{draft.body}</div>
              <button
                type="button"
                className="drafts-panel__move"
                onClick={() => onReanchor(draft.path, draft.side, draft.line, cursor)}
              >
                Move to row {cursor + 1}
              </button>
            </div>
          ))}
        </>
      )}
    </div>
  );
}
