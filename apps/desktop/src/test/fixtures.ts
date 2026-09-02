import type { FileDetailDto, FileSummaryDto, RowDto, SessionSnapshotDto, SidebarDto } from "../bindings";

export function makeRow(overrides: Partial<RowDto> = {}): RowDto {
  return {
    kind: "Context",
    old_line: 1,
    new_line: 1,
    text: "line",
    hunk_header: null,
    thread_count: 0,
    has_draft: false,
    draft_is_proposed: false,
    ...overrides,
  };
}

export function makeFile(overrides: Partial<FileDetailDto> = {}): FileDetailDto {
  return {
    index: 0,
    path: "src/review_fixture_00.rs",
    rows: [makeRow()],
    ...overrides,
  };
}

export function makeFileSummary(overrides: Partial<FileSummaryDto> = {}): FileSummaryDto {
  return {
    index: 0,
    path: "src/review_fixture_00.rs",
    old_path: null,
    status: "Modified",
    is_binary: false,
    additions: 1,
    deletions: 0,
    viewed: false,
    thread_count: 0,
    ...overrides,
  };
}

export function makeSidebar(files: FileSummaryDto[] = [makeFileSummary()]): SidebarDto {
  return {
    files,
    selected_file: 0,
    viewed_count: files.filter((file) => file.viewed).length,
    thread_count: files.reduce((sum, file) => sum + file.thread_count, 0),
  };
}

export function makeSnapshot(overrides: Partial<SessionSnapshotDto> = {}): SessionSnapshotDto {
  return {
    title: "Generated fixture",
    subtitle: "Diff virtualization demo",
    sidebar: makeSidebar(),
    ...overrides,
  };
}
