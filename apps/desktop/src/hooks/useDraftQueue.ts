import { useCallback, useRef } from "react";
import type { DiffSideDto, DraftsDto } from "../bindings";
import { commands } from "../bindings";
import type { SessionAction } from "./sessionReducer";

type DraftCommand =
  | { kind: "edit"; fileIndex: number; start: number; end: number; body: string }
  | { kind: "discard"; fileIndex: number; row: number }
  | { kind: "reanchor"; fileIndex: number; path: string; side: DiffSideDto; line: number; row: number }
  | { kind: "summary"; body: string };

function send(command: DraftCommand) {
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

/** Whichever `DraftsDto` a settled command hands back, and whether it was accepted. */
function outcome(command: DraftCommand, data: DraftsDto | { accepted: boolean; drafts: DraftsDto }) {
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

/** Serialises draft edits, discards, reanchors, and summary writes, at most one in flight, consecutive writes of one text coalescing while a discard or reanchor is always sent in order. */
export function useDraftQueue(dispatch: (action: SessionAction) => void) {
  const inFlight = useRef(false);
  const pending = useRef<DraftCommand[]>([]);

  const settle = useCallback(
    (command: DraftCommand, data: DraftsDto | { accepted: boolean; drafts: DraftsDto } | null) => {
      // The summary answers with nothing. The editor already holds what it sent.
      if (command.kind === "summary" || data === null) {
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

  return { editDraft, discardDraft, reanchorDraft, editSummary };
}
