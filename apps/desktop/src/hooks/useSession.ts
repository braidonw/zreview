import { useCallback, useEffect, useReducer, useRef } from "react";
import { Channel } from "@tauri-apps/api/core";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import type { DiffSideDto, SessionFailureDto } from "../bindings";
import { commands } from "../bindings";
import { clamp } from "../lib/clamp";
import { initialState, selectionRange, sessionReducer } from "./sessionReducer";
import { useDraftQueue } from "./useDraftQueue";

/** Narrows a command rejection into a SessionFailureDto. */
function toFailure(error: unknown): SessionFailureDto {
  if (
    typeof error === "object" &&
    error !== null &&
    "summary" in error &&
    typeof (error as { summary: unknown }).summary === "string"
  ) {
    return error as SessionFailureDto;
  }
  return { summary: String(error), detail: null, remediation: null };
}

/** Loads the session the window was launched into and exposes every action the UI can take on it. */
export function useSession(description: string) {
  const [state, dispatch] = useReducer(sessionReducer, initialState);
  const stateRef = useRef(state);
  stateRef.current = state;
  const hasOpened = useRef(false);
  const draftQueue = useDraftQueue(dispatch);

  useEffect(() => {
    if (hasOpened.current) {
      return;
    }
    hasOpened.current = true;
    dispatch({ type: "describe", description });

    const channel = new Channel<string>();
    channel.onmessage = (stage) => dispatch({ type: "stage", stage });

    commands
      .openSession(channel)
      .then((opened) => {
        if (opened.status === "error") {
          dispatch({ type: "failed", failure: toFailure(opened.error) });
          return;
        }
        const snapshot = opened.data;
        commands.selectFile(0).then((selected) => {
          if (selected.status === "error") {
            dispatch({ type: "failed", failure: toFailure(selected.error) });
            return;
          }
          dispatch({ type: "ready", snapshot, file: selected.data });
        });
      })
      .catch((error: unknown) => {
        dispatch({ type: "failed", failure: toFailure(error) });
      });
  }, [description]);

  const selectFile = useCallback((index: number) => {
    commands.selectFile(index).then((selected) => {
      if (selected.status === "error") {
        dispatch({ type: "failed", failure: toFailure(selected.error) });
        return;
      }
      dispatch({ type: "file", file: selected.data });
    });
  }, []);

  const toggleViewed = useCallback(() => {
    commands.toggleViewed().then((toggled) => {
      if (toggled.status === "error") {
        dispatch({ type: "failed", failure: toFailure(toggled.error) });
        return;
      }
      dispatch({ type: "sidebar", sidebar: toggled.data });
    });
  }, []);

  const clickRow = useCallback((index: number) => dispatch({ type: "click", index }), []);

  const openComposer = useCallback((index: number) => dispatch({ type: "openComposer", index }), []);

  const closeComposer = useCallback(() => dispatch({ type: "closeComposer" }), []);

  /** Persists what the composer holds, over whatever span it was opened on. */
  const composerChange = useCallback(
    (body: string) => {
      const current = stateRef.current;
      if (current.status !== "ready" || !current.composer) {
        return;
      }
      const [start, end] = current.composer.rows;
      draftQueue.editDraft(current.drafts.file_index, start, end, body);
    },
    [draftQueue],
  );

  /** Discards the draft the composer is open on, then closes it. */
  const composerDiscard = useCallback(() => {
    const current = stateRef.current;
    if (current.status !== "ready" || !current.composer) {
      return;
    }
    draftQueue.discardDraft(current.drafts.file_index, current.composer.rows[1]);
    dispatch({ type: "closeComposer" });
  }, [draftQueue]);

  const reanchorDraft = useCallback(
    (path: string, side: DiffSideDto, line: number, row: number) => {
      const current = stateRef.current;
      if (current.status !== "ready") {
        return;
      }
      draftQueue.reanchorDraft(current.drafts.file_index, path, side, line, row);
    },
    [draftQueue],
  );

  useEffect(() => {
    if (state.status !== "ready") {
      return;
    }

    function handleKeydown(event: KeyboardEvent) {
      // The composer is a real text editor, so global shortcuts yield to it entirely.
      if (event.target instanceof HTMLElement && event.target.closest("[data-composer]")) {
        return;
      }

      const current = stateRef.current;
      if (current.status !== "ready") {
        return;
      }
      const key = event.key.toLowerCase();

      if (event.metaKey && event.shiftKey && key === "j") {
        event.preventDefault();
        const { selected_file, files } = current.snapshot.sidebar;
        selectFile(clamp(selected_file + 1, 0, files.length - 1));
        return;
      }
      if (event.metaKey && event.shiftKey && key === "k") {
        event.preventDefault();
        const { selected_file, files } = current.snapshot.sidebar;
        selectFile(clamp(selected_file - 1, 0, files.length - 1));
        return;
      }
      if (event.metaKey && event.shiftKey && key === "v") {
        event.preventDefault();
        toggleViewed();
        return;
      }
      if (event.metaKey && !event.shiftKey && key === "c") {
        event.preventDefault();
        const [start, end] = selectionRange(current);
        const text = current.file.rows
          .slice(start, end + 1)
          .map((row) => row.text)
          .join("\n");
        void writeText(text);
        return;
      }
      if (!event.metaKey && key === "c") {
        event.preventDefault();
        dispatch({ type: "toggleComposer" });
        return;
      }
      if (!event.metaKey && key === "j") {
        event.preventDefault();
        dispatch({ type: "move", delta: 1, extend: event.shiftKey });
        return;
      }
      if (!event.metaKey && key === "k") {
        event.preventDefault();
        dispatch({ type: "move", delta: -1, extend: event.shiftKey });
      }
    }

    window.addEventListener("keydown", handleKeydown);
    return () => window.removeEventListener("keydown", handleKeydown);
  }, [state.status, selectFile, toggleViewed]);

  return {
    state,
    selectFile,
    clickRow,
    openComposer,
    closeComposer,
    composerChange,
    composerDiscard,
    reanchorDraft,
  };
}
