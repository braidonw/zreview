import type {
  AnchoredDraftDto,
  DraftsDto,
  FileDetailDto,
  FileSummaryDto,
  FindingDto,
  GuidanceDto,
  GuidanceEntryDto,
  HomeGroupDto,
  HomeRepositoryDto,
  HomeRowDto,
  HomeSnapshotDto,
  ReviewPanelDto,
  RowDto,
  SessionSnapshotDto,
  SidebarDto,
  StaleDraftDto,
  SubmissionDto,
  SubmissionRequestDto,
} from "../bindings";

/** The guidance section once discovery has found something. */
type DiscoveredGuidance = Extract<GuidanceDto, { kind: "Discovered" }>;

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
    ready_count: 0,
    not_anchored_count: 0,
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
    can_submit: false,
    summary: "",
    ...overrides,
  };
}

export function makeSubmissionRequest(
  overrides: Partial<SubmissionRequestDto> = {},
): SubmissionRequestDto {
  return {
    heading: "Comment with 1 inline comment",
    pinned: "pinned to abc1234",
    body: "Two notes.",
    comments: [{ position: "src/review_fixture_00.rs RIGHT line 2", body: "needs a test" }],
    excluded: [],
    excluded_heading: null,
    ...overrides,
  };
}

/** The submission at whatever phase a test needs, one revision on from idle. */
export function makeSubmission(
  phase: SubmissionDto["phase"] = { state: "Idle" },
  revision = 1,
): SubmissionDto {
  return { revision, phase };
}

export function makeGuidanceEntry(overrides: Partial<GuidanceEntryDto> = {}): GuidanceEntryDto {
  return {
    path: "AGENTS.md",
    scope: "whole repository",
    kilobytes: 2,
    included: true,
    ...overrides,
  };
}

export function makeGuidance(overrides: Partial<DiscoveredGuidance> = {}): GuidanceDto {
  return {
    kind: "Discovered",
    summary: "1 guidance file · 2 KB",
    expanded: true,
    entries: [makeGuidanceEntry()],
    skipped: [],
    excluded: null,
    ...overrides,
  };
}

export function makeFinding(overrides: Partial<FindingDto> = {}): FindingDto {
  return {
    id: 1,
    severity: "Warning",
    confidence_percent: 90,
    title: "Unchecked index",
    rationale: "This can panic on an empty slice.",
    citations: ["AGENTS.md"],
    origin: "claude-code",
    position: "src/review_fixture_00.rs:2",
    is_selected: false,
    ...overrides,
  };
}

export function makePanel(overrides: Partial<ReviewPanelDto> = {}): ReviewPanelDto {
  return {
    revision: 1,
    heading: "Review",
    guidance: makeGuidance(),
    run: { state: "Idle" },
    note: {
      heading: "No review has been run.",
      detail: "Press Review to check this change against the repository's guidance.",
    },
    findings: [],
    footer: null,
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
    repository: "acme/widgets",
    number: 412,
    identity: "acme/widgets#412",
    author: "mlee",
    updated_at_ms: 1_788_266_096_000,
    review_status: null,
    check_status: null,
    drafts_label: null,
    is_alive: false,
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
    drafts_failure: null,
    ...overrides,
  };
}
