import type {
  AnchoredDraftDto,
  DraftsDto,
  FileDetailDto,
  FileSummaryDto,
  HomeRepositoryDto,
  HomeSnapshotDto,
  RowDto,
  SessionSnapshotDto,
  SidebarDto,
  StaleDraftDto,
} from "../bindings";

export function makeRow(overrides: Partial<RowDto> = {}): RowDto {
  return {
    kind: "Context",
    old_line: 1,
    new_line: 1,
    text: "line",
    hunk_header: null,
    thread_count: 0,
    ...overrides,
  };
}

export function makeAnchoredDraft(overrides: Partial<AnchoredDraftDto> = {}): AnchoredDraftDto {
  return {
    row: 0,
    body: "a draft",
    is_proposed: false,
    ...overrides,
  };
}

export function makeStaleDraft(overrides: Partial<StaleDraftDto> = {}): StaleDraftDto {
  return {
    path: "src/review_fixture_00.rs",
    side: "Right",
    line: 9999,
    body: "written last week",
    location: "was RIGHT line 9999",
    ...overrides,
  };
}

export function makeDrafts(overrides: Partial<DraftsDto> = {}): DraftsDto {
  return {
    file_index: 0,
    anchored: [],
    stale: [],
    file_draft_count: 0,
    write_failure: null,
    ...overrides,
  };
}

export function makeFile(overrides: Partial<FileDetailDto> = {}): FileDetailDto {
  return {
    index: 0,
    path: "src/review_fixture_00.rs",
    rows: [makeRow()],
    drafts: makeDrafts(),
    empty_reason: null,
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
    warnings: [],
    ...overrides,
  };
}

export function makeHomeRepository(overrides: Partial<HomeRepositoryDto> = {}): HomeRepositoryDto {
  return {
    path: "/Developer/zreview",
    slug: "braidonw/zreview",
    failure: null,
    ...overrides,
  };
}

export function makeHomeSnapshot(overrides: Partial<HomeSnapshotDto> = {}): HomeSnapshotDto {
  return {
    count_line: null,
    groups: [
      { title: "To review", count: 0, empty_copy: "Nothing waiting for your review." },
      { title: "To address", count: 0, empty_copy: "Nothing to address." },
      { title: "Waiting on others", count: 0, empty_copy: "Nothing waiting on others." },
    ],
    repositories: [],
    footer_summary: "No repositories",
    footer_expanded: false,
    refusals: [],
    failure: null,
    write_failure: null,
    ...overrides,
  };
}
