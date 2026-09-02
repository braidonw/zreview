import type { DiffSideDto, FileStatusDto, FileSummaryDto, SessionFailureDto, SidebarDto, StaleDraftDto } from "../bindings";
import { DraftsPanel } from "./DraftsPanel";
import "./FileSidebar.css";

const STATUS_GLYPH: Record<FileStatusDto, { label: string; className: string }> = {
  Added: { label: "A", className: "file-row__status--success" },
  Deleted: { label: "D", className: "file-row__status--error" },
  Modified: { label: "M", className: "file-row__status--warning" },
  Renamed: { label: "R", className: "file-row__status--info" },
  Copied: { label: "C", className: "file-row__status--proposed" },
  TypeChanged: { label: "T", className: "file-row__status--warning" },
  Unmerged: { label: "U", className: "file-row__status--error-strong" },
};

export function FileSidebar({
  title,
  subtitle,
  sidebar,
  warnings,
  writeFailure,
  fileDraftCount,
  staleDrafts,
  cursor,
  onSelect,
  onReanchorDraft,
}: {
  title: string;
  subtitle: string;
  sidebar: SidebarDto;
  warnings: SessionFailureDto[];
  writeFailure: string | null;
  fileDraftCount: number;
  staleDrafts: StaleDraftDto[];
  cursor: number;
  onSelect: (index: number) => void;
  onReanchorDraft: (path: string, side: DiffSideDto, line: number, row: number) => void;
}) {
  const warningMessages = warnings.map((warning) => warning.summary);
  if (writeFailure !== null) {
    warningMessages.push(writeFailure);
  }

  return (
    <div className="file-sidebar">
      <div className="file-sidebar__header">
        <div className="file-sidebar__label">{title}</div>
        <div className="file-sidebar__title">{subtitle}</div>
        <div className="file-sidebar__meta">
          {sidebar.files.length} files &middot; {sidebar.viewed_count} viewed &middot;{" "}
          {sidebar.thread_count} conversations
        </div>
      </div>
      {warningMessages.length > 0 && (
        <div className="file-sidebar__warnings">
          {warningMessages.map((message) => (
            <div key={message} className="file-sidebar__warning">
              {message}
            </div>
          ))}
        </div>
      )}
      <div className="file-sidebar__list">
        {sidebar.files.map((file) => (
          <FileRow
            key={file.index}
            file={file}
            selected={file.index === sidebar.selected_file}
            onSelect={onSelect}
          />
        ))}
      </div>
      <DraftsPanel
        fileDraftCount={fileDraftCount}
        stale={staleDrafts}
        cursor={cursor}
        onReanchor={onReanchorDraft}
      />
    </div>
  );
}

function FileRow({
  file,
  selected,
  onSelect,
}: {
  file: FileSummaryDto;
  selected: boolean;
  onSelect: (index: number) => void;
}) {
  const status = STATUS_GLYPH[file.status];

  return (
    <div
      className={`file-row ${selected ? "file-row--selected" : ""} ${
        file.viewed ? "file-row--viewed" : ""
      }`}
      onClick={() => onSelect(file.index)}
    >
      <span className={`file-row__status ${status.className}`}>{status.label}</span>
      <span className="file-row__path">{file.path}</span>
      {file.thread_count > 0 && <span className="file-row__threads">{file.thread_count}</span>}
      {file.is_binary ? (
        <span className="file-row__binary">binary</span>
      ) : (
        <span className="file-row__counts">
          <span className="file-row__additions">+{file.additions}</span>
          <span className="file-row__deletions">-{file.deletions}</span>
        </span>
      )}
      {file.viewed && <span className="file-row__check">&#10003;</span>}
    </div>
  );
}
