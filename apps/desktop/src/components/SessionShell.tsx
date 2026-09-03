import type { DiffSideDto } from "../bindings";
import type { ReadyState } from "../hooks/sessionReducer";
import { composerPrefill, selectionRange } from "../hooks/sessionReducer";
import { DiffList } from "./DiffList";
import { EmptyDiffPane } from "./EmptyDiffPane";
import { FileSidebar } from "./FileSidebar";
import "./SessionShell.css";

export function SessionShell({
  state,
  onBack,
  onSelectFile,
  onRowClick,
  onOpenComposer,
  onComposerChange,
  onComposerClose,
  onComposerDiscard,
  onReanchorDraft,
}: {
  state: ReadyState;
  /** Absent for a Session with no Home behind it, which offers no way back. */
  onBack: (() => void) | null;
  onSelectFile: (index: number) => void;
  onRowClick: (index: number) => void;
  onOpenComposer: (index: number) => void;
  onComposerChange: (body: string) => void;
  onComposerClose: () => void;
  onComposerDiscard: () => void;
  onReanchorDraft: (path: string, side: DiffSideDto, line: number, row: number) => void;
}) {
  const [selectionStart, selectionEnd] = selectionRange(state);
  const { empty_reason: emptyReason, rows } = state.file;
  const isEmpty = emptyReason !== null || rows.length === 0;

  return (
    <div className="session-shell">
      <FileSidebar
        onBack={onBack}
        title={state.snapshot.title}
        subtitle={state.snapshot.subtitle}
        sidebar={state.snapshot.sidebar}
        warnings={state.snapshot.warnings}
        writeFailure={state.drafts.write_failure}
        fileDraftCount={state.drafts.file_draft_count}
        staleDrafts={state.drafts.stale}
        cursor={state.cursor}
        onSelect={onSelectFile}
        onReanchorDraft={onReanchorDraft}
      />
      {isEmpty ? (
        <EmptyDiffPane
          label={emptyReason?.label ?? "No lines to show"}
          detail={emptyReason?.detail ?? "This file has nothing to display."}
        />
      ) : (
        <DiffList
          rows={rows}
          fileIndex={state.file.index}
          cursor={state.cursor}
          selectionStart={selectionStart}
          selectionEnd={selectionEnd}
          drafts={state.drafts}
          composer={state.composer}
          composerPrefill={composerPrefill(state)}
          onRowClick={onRowClick}
          onOpenComposer={onOpenComposer}
          onComposerChange={onComposerChange}
          onComposerClose={onComposerClose}
          onComposerDiscard={onComposerDiscard}
        />
      )}
    </div>
  );
}
