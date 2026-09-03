import { useCallback, useEffect, useReducer, useRef } from "react";
import { Channel } from "@tauri-apps/api/core";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import type { DiffSideDto, ReviewPanelDto } from "../bindings";
import { commands } from "../bindings";
import { clamp } from "../lib/clamp";
import { toFailure } from "../lib/failure";
import { initialState, selectionRange, sessionReducer } from "./sessionReducer";
import { useDraftQueue } from "./useDraftQueue";

/**
 * Loads one session and exposes every action the UI can take on it.
 *
 * `isShowing` is false while Home is in front of this session, which keeps its
 * hidden tree from answering keystrokes meant for the list.
 */
export function useSession(description: string, isShowing: boolean) {
  const [state, dispatch] = useReducer(sessionReducer, initialState);
  const stateRef = useRef(state);
  stateRef.current = state;
  const hasOpened = useRef(false);
  const draftQueue = useDraftQueue(dispatch);
  // Held here rather than read off the panel, because a second trigger can arrive
  // before the first run's own "Running" panel has come back over the channel.
  const isRunning = useRef(false);

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
        Promise.all([commands.selectFile(0), commands.reviewPanel()]).then(([selected, panel]) => {
          if (selected.status === "error") {
            dispatch({ type: "failed", failure: toFailure(selected.error) });
            return;
          }
          if (panel.status === "error") {
            dispatch({ type: "failed", failure: toFailure(panel.error) });
            return;
          }
          dispatch({ type: "ready", snapshot, file: selected.data, panel: panel.data });
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

  /**
   * Starts a review, unless one is already in flight.
   *
   * The run holds the session in the backend for as long as the coding agent
   * takes, so the panel arrives in pieces: the running state and every progress
   * line over the channel, the outcome when the promise settles.
   */
  const runReview = useCallback(() => {
    if (isRunning.current) {
      return;
    }
    isRunning.current = true;

    const channel = new Channel<ReviewPanelDto>();
    channel.onmessage = (panel) => dispatch({ type: "panel", panel });

    commands
      .runReview(channel)
      .then((finished) => {
        isRunning.current = false;
        if (finished.status === "error") {
          dispatch({ type: "failed", failure: toFailure(finished.error) });
          return;
        }
        dispatch({ type: "panel", panel: finished.data });
      })
      .catch((error: unknown) => {
        isRunning.current = false;
        dispatch({ type: "failed", failure: toFailure(error) });
      });
  }, []);

  /** Asks the running review to stop. It ends at the backend's next step. */
  const cancelReview = useCallback(() => {
    commands.cancelReview().then((cancelled) => {
      if (cancelled.status === "error") {
        dispatch({ type: "failed", failure: toFailure(cancelled.error) });
        return;
      }
      dispatch({ type: "panel", panel: cancelled.data });
    });
  }, []);

  const toggleGuidanceSection = useCallback(() => {
    commands.toggleGuidancePanel().then((toggled) => {
      if (toggled.status === "error") {
        dispatch({ type: "failed", failure: toFailure(toggled.error) });
        return;
      }
      dispatch({ type: "panel", panel: toggled.data });
    });
  }, []);

  const toggleGuidanceFile = useCallback((path: string) => {
    commands.toggleGuidance(path).then((toggled) => {
      if (toggled.status === "error") {
        dispatch({ type: "failed", failure: toFailure(toggled.error) });
        return;
      }
      dispatch({ type: "panel", panel: toggled.data });
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
    if (state.status !== "ready" || !isShowing) {
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
      if (event.metaKey && event.shiftKey && key === "r") {
        event.preventDefault();
        runReview();
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
  }, [state.status, isShowing, selectFile, toggleViewed, runReview]);

  return {
    state,
    selectFile,
    clickRow,
    openComposer,
    closeComposer,
    composerChange,
    composerDiscard,
    reanchorDraft,
    runReview,
    cancelReview,
    toggleGuidanceSection,
    toggleGuidanceFile,
  };
}
