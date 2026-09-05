import { useCallback, useRef } from "react";
import type { DiffSideDto, DraftsDto } from "../bindings";
import { commands } from "../bindings";
import type { SessionAction } from "./sessionReducer";

type DraftCommand =
  | { kind: "edit"; fileIndex: number; start: number; end: number; body: string }
  | { kind: "discard"; fileIndex: number; row: number }
  | { kind: "reanchor"; fileIndex: number; path: string; side: DiffSideDto; line: number; row: number }
  | { kind: "summary"; body: string }
  /** Work that must not be overtaken by the writes above, reporting for itself. */
  | { kind: "job"; run: () => Promise<unknown> };

/** Everything but a job, which the queue runs rather than sends. */
type SentCommand = Exclude<DraftCommand, { kind: "job" }>;

function send(command: SentCommand) {
  switch (command.kind) {
    case "edit":
      return commands.editDraft(command.fileIndex, command.start, command.end, command.body);
    case "discard":
      return commands.discardDraft(command.fileIndex, command.row);
    case "reanchor":
      return commands.reanchorDraft(
        command.fileIndex,
        command.path,
        command.side,
        command.line,
        command.row,
      );
    case "summary":
      return commands.editSummary(command.body);
  }
}

/** Whatever a settled command hands back, before it is narrowed by its kind. */
type CommandData = DraftsDto | { accepted: boolean; drafts: DraftsDto } | string | null;

/** Whichever `DraftsDto` a settled draft command hands back, and whether it was accepted. */
function outcome(command: SentCommand, data: CommandData) {
  return command.kind === "edit"
    ? (data as { accepted: boolean; drafts: DraftsDto })
    : { accepted: true, drafts: data as DraftsDto };
}

/** Whether two consecutive commands may collapse into the later one, which only per-keystroke writes of the same text may. */
function coalesces(command: DraftCommand, previous: DraftCommand | undefined): boolean {
  if (command.kind === "edit") {
    return previous?.kind === "edit";
  }
  if (command.kind === "summary") {
    return previous?.kind === "summary";
  }
  return false;
}

/** Serialises draft edits, discards, reanchors, summary writes, and any job that must not be overtaken by them, at most one in flight, consecutive writes of one text coalescing while everything else is sent in order. */
export function useDraftQueue(dispatch: (action: SessionAction) => void) {
  const inFlight = useRef(false);
  const pending = useRef<DraftCommand[]>([]);

  const settle = useCallback(
    (command: SentCommand, data: CommandData) => {
      // A summary write does not echo the text back, because the editor already
      // holds it. What it answers with is whatever is stopping writes landing,
      // which for a reviewer touching no draft is the only way they are told.
      if (command.kind === "summary") {
        dispatch({ type: "writeFailure", failure: data as string | null });
        return;
      }
      if (data === null) {
        return;
      }
      const { accepted, drafts } = outcome(command, data);
      dispatch({ type: "drafts", drafts });
      if (!accepted) {
        dispatch({ type: "editRejected" });
      }
    },
    [dispatch],
  );

  const run = useCallback(
    (command: DraftCommand) => {
      const advance = () => {
        inFlight.current = false;
        const next = pending.current.shift();
        if (next) {
          run(next);
        }
      };
      inFlight.current = true;
      if (command.kind === "job") {
        // The job answers to its caller; the queue only holds the slot so no
        // write it was meant to follow can land on top of it.
        command.run().then(advance, advance);
        return;
      }
      send(command).then((result) => {
        if (result.status === "ok") {
          settle(command, result.data);
        }
        // A draft-command failure, or a rejected send, is just dropped, never fatal.
        advance();
      }, advance);
    },
    [settle],
  );

  const enqueue = useCallback(
    (command: DraftCommand) => {
      if (!inFlight.current) {
        run(command);
        return;
      }
      const queue = pending.current;
      if (coalesces(command, queue[queue.length - 1])) {
        queue[queue.length - 1] = command;
      } else {
        queue.push(command);
      }
    },
    [run],
  );

  const editDraft = useCallback(
    (fileIndex: number, start: number, end: number, body: string) =>
      enqueue({ kind: "edit", fileIndex, start, end, body }),
    [enqueue],
  );

  const discardDraft = useCallback(
    (fileIndex: number, row: number) => enqueue({ kind: "discard", fileIndex, row }),
    [enqueue],
  );

  const reanchorDraft = useCallback(
    (fileIndex: number, path: string, side: DiffSideDto, line: number, row: number) =>
      enqueue({ kind: "reanchor", fileIndex, path, side, line, row }),
    [enqueue],
  );

  const editSummary = useCallback((body: string) => enqueue({ kind: "summary", body }), [enqueue]);

  /** Runs `work` once every write queued before it has landed, holding the queue while it does. */
  const runInOrder = useCallback(
    (work: () => Promise<unknown>) => enqueue({ kind: "job", run: work }),
    [enqueue],
  );

  return { editDraft, discardDraft, reanchorDraft, editSummary, runInOrder };
}
