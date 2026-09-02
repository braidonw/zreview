import type { ReadyState } from "../hooks/sessionReducer";
import { selectionRange } from "../hooks/sessionReducer";
import { DiffList } from "./DiffList";
import { FileSidebar } from "./FileSidebar";
import "./SessionShell.css";

export function SessionShell({
  state,
  onSelectFile,
  onRowClick,
}: {
  state: ReadyState;
  onSelectFile: (index: number) => void;
  onRowClick: (index: number) => void;
}) {
  const [selectionStart, selectionEnd] = selectionRange(state);

  return (
    <div className="session-shell">
      <FileSidebar
        title={state.snapshot.title}
        subtitle={state.snapshot.subtitle}
        sidebar={state.snapshot.sidebar}
        onSelect={onSelectFile}
      />
      <DiffList
        rows={state.file.rows}
        fileIndex={state.file.index}
        cursor={state.cursor}
        selectionStart={selectionStart}
        selectionEnd={selectionEnd}
        onRowClick={onRowClick}
      />
    </div>
  );
}
