import type { AnchoredDraftDto, DiffLineKindDto, RowDto } from "../bindings";
import "./DiffRow.css";

const KIND_CLASS: Record<DiffLineKindDto, string> = {
  Context: "diff-row--context",
  Addition: "diff-row--addition",
  Deletion: "diff-row--deletion",
  NoNewlineMarker: "diff-row--no-newline",
};

const KIND_MARKER: Record<DiffLineKindDto, string> = {
  Context: " ",
  Addition: "+",
  Deletion: "-",
  NoNewlineMarker: "\\",
};

export function DiffRow({
  row,
  selected,
  inSelection,
  draft,
  showPill,
  onClick,
  onOpenComposer,
}: {
  row: RowDto;
  selected: boolean;
  inSelection: boolean;
  draft?: AnchoredDraftDto;
  /** Shown only at the cursor's own row, while the composer is not open there. */
  showPill?: boolean;
  onClick: () => void;
  onOpenComposer?: () => void;
}) {
  const rail = railClass(row, selected, inSelection, draft);

  return (
    <div
      className={`diff-row ${KIND_CLASS[row.kind]} ${selected ? "diff-row--selected" : ""} ${
        inSelection ? "diff-row--in-selection" : ""
      }`}
      onClick={onClick}
    >
      <div className={`diff-row__rail ${rail}`} />
      <div className="diff-row__gutter diff-row__gutter--old">{row.old_line ?? ""}</div>
      <div className="diff-row__gutter diff-row__gutter--new">{row.new_line ?? ""}</div>
      <div className="diff-row__marker">{KIND_MARKER[row.kind]}</div>
      <div className="diff-row__text">{row.text}</div>
      {draft && <span className="diff-row__draft-chip">draft</span>}
      {showPill && (
        <button
          type="button"
          className="diff-row__pill"
          onClick={(event) => {
            event.stopPropagation();
            onOpenComposer?.();
          }}
        >
          {draft ? "Edit" : "Comment"}
        </button>
      )}
    </div>
  );
}

function railClass(
  row: RowDto,
  selected: boolean,
  inSelection: boolean,
  draft: AnchoredDraftDto | undefined,
): string {
  if (selected || inSelection) {
    return "diff-row__rail--accent";
  }
  if (draft) {
    return draft.is_proposed ? "diff-row__rail--proposed" : "diff-row__rail--accent-dim";
  }
  if (row.thread_count > 0) {
    return "diff-row__rail--thread";
  }
  return "";
}
