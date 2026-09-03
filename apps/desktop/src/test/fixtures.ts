import type {
  AnchoredDraftDto,
  DraftsDto,
  FileDetailDto,
  FileSummaryDto,
  HomeGroupDto,
  HomeRepositoryDto,
  HomeRowDto,
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

export function makeHomeRow(overrides: Partial<HomeRowDto> = {}): HomeRowDto {
  return {
    index: 0,
    title: "Retry webhook deliveries with jittered backoff",
    url: "https://github.com/acme/widgets/pull/412",
    identity: "acme/widgets#412",
    author: "mlee",
    updated_at_ms: 1_788_266_096_000,
    review_status: null,
    check_status: null,
    ...overrides,
  };
}

/** The three groups in their fixed order, holding whatever rows are given. */
export function makeHomeGroups(rows: HomeRowDto[][] = [[], [], []]): HomeGroupDto[] {
  const shape = [
    { title: "To review", empty_copy: "Nothing waiting for your review." },
    { title: "To address", empty_copy: "Nothing to address." },
    { title: "Waiting on others", empty_copy: "Nothing waiting on others." },
  ];
  return shape.map((group, index) => ({
    ...group,
    count: rows[index].length,
    rows: rows[index],
  }));
}

export function makeHomeSnapshot(overrides: Partial<HomeSnapshotDto> = {}): HomeSnapshotDto {
  return {
    count_line: null,
    groups: makeHomeGroups(),
    cursor: 0,
    refresh: "NeverRefreshed",
    failed_repositories: [],
    repositories: [],
    footer_summary: "No repositories",
    footer_expanded: false,
    refusals: [],
    failure: null,
    write_failure: null,
    ...overrides,
  };
}
