import { useCallback, useEffect, useReducer, useRef } from "react";
import { Channel } from "@tauri-apps/api/core";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import type { DiffSideDto, FindingLocationDto, ReviewPanelDto } from "../bindings";
import { commands } from "../bindings";
import { clamp } from "../lib/clamp";
import { toFailure } from "../lib/failure";
import { initialState, selectedFindingId, selectionRange, sessionReducer } from "./sessionReducer";
import { useDraftQueue } from "./useDraftQueue";

/** What a command answering with the review panel hands back. */
type PanelResult =
  | { status: "ok"; data: ReviewPanelDto | null }
  | { status: "error"; error: unknown };

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

    try {
      const channel = new Channel<ReviewPanelDto>();
      channel.onmessage = (panel) => dispatch({ type: "panel", panel });

      commands
        .runReview(channel)
        .then((finished) => {
          if (finished.status === "error") {
            dispatch({ type: "panelNotice", notice: toFailure(finished.error).summary });
            return;
          }
          dispatch({ type: "panel", panel: finished.data });
        })
        .catch((error: unknown) => {
          dispatch({ type: "panelNotice", notice: toFailure(error).summary });
        })
        .finally(() => {
          isRunning.current = false;
        });
    } catch (error: unknown) {
      // A throw before the promise exists would otherwise leave the flag set and
      // no review startable for the rest of the sitting.
      isRunning.current = false;
      dispatch({ type: "panelNotice", notice: toFailure(error).summary });
    }
  }, []);

  /**
   * Runs one of the panel's own commands, showing a refusal inside the panel.
   *
   * These are about the review, not the sitting. Replacing the Session with a
   * failure screen would throw away the diff and whatever is unsent in the
   * composer, over a toggle that did not take.
   */
  const onPanel = useCallback(
    (call: Promise<PanelResult>) => {
      call
        .then((answered) => {
          if (answered.status === "error") {
            dispatch({ type: "panelNotice", notice: toFailure(answered.error).summary });
            return;
          }
          dispatch({ type: "panel", panel: answered.data });
        })
        .catch((error: unknown) => {
          dispatch({ type: "panelNotice", notice: toFailure(error).summary });
        });
    },
    [],
  );

  /** Asks the running review to stop. It ends at the backend's next step. */
  const cancelReview = useCallback(() => onPanel(commands.cancelReview()), [onPanel]);

  const toggleGuidanceSection = useCallback(
    () => onPanel(commands.toggleGuidancePanel()),
    [onPanel],
  );

  const toggleGuidanceFile = useCallback(
    (path: string) => onPanel(commands.toggleGuidance(path)),
    [onPanel],
  );

  /**
   * Switches to a finding's file and selects its row, if it has one.
   *
   * A finding about the change as a whole has nowhere to scroll to, so the panel
   * selection is the only thing that moves.
   */
  const applyLocation = useCallback((location: FindingLocationDto | null) => {
    if (location === null) {
      return;
    }
    const current = stateRef.current;
    if (current.status === "ready" && current.file.index === location.file) {
      dispatch({ type: "click", index: location.row });
      return;
    }
    commands.selectFile(location.file).then((selected) => {
      if (selected.status === "error") {
        dispatch({ type: "panelNotice", notice: toFailure(selected.error).summary });
        return;
      }
      dispatch({ type: "file", file: selected.data });
      dispatch({ type: "click", index: location.row });
    });
  }, []);

  /** Selects a finding, scrolling the diff to its anchor and selecting the row. */
  const revealFinding = useCallback(
    (id: number) => {
      commands.revealFinding(id).then((result) => {
        if (result.status === "error") {
          dispatch({ type: "panelNotice", notice: toFailure(result.error).summary });
          return;
        }
        if (result.data === null) {
          return;
        }
        dispatch({ type: "panel", panel: result.data.panel });
        applyLocation(result.data.location);
      });
    },
    [applyLocation],
  );

  /** Moves the selection to the finding after the one selected, wrapping to the first. */
  const selectNextFinding = useCallback(() => {
    commands.selectNextFinding().then((result) => {
      if (result.status === "error") {
        dispatch({ type: "panelNotice", notice: toFailure(result.error).summary });
        return;
      }
      if (result.data === null) {
        return;
      }
      dispatch({ type: "panel", panel: result.data.panel });
      applyLocation(result.data.location);
    });
  }, [applyLocation]);

  /**
   * Accepts a finding, turning it into a draft where the model allows it.
   *
   * When the anchor already holds the reviewer's own draft, neither text is
   * written; the panel asks whether to replace it instead.
   */
  const acceptFinding = useCallback((id: number) => {
    commands.acceptFinding(id).then((result) => {
      if (result.status === "error") {
        dispatch({ type: "panelNotice", notice: toFailure(result.error).summary });
        return;
      }
      if (result.data === null) {
        return;
      }
      const { panel, drafts, disposition } = result.data;
      dispatch({ type: "panel", panel });
      dispatch({ type: "drafts", drafts });
      dispatch({
        type: "findingConflict",
        conflict:
          disposition.outcome === "Occupied"
            ? { id, existing: disposition.existing, proposed: disposition.proposed }
            : null,
      });
    });
  }, []);

  const dismissFinding = useCallback(
    (id: number) => {
      dispatch({ type: "findingConflict", conflict: null });
      onPanel(commands.dismissFinding(id));
    },
    [onPanel],
  );

  /** Forces a finding onto its anchor, overwriting the reviewer's own draft there. */
  const replaceFinding = useCallback((id: number) => {
    commands.overwriteFinding(id).then((result) => {
      dispatch({ type: "findingConflict", conflict: null });
      if (result.status === "error") {
        dispatch({ type: "panelNotice", notice: toFailure(result.error).summary });
        return;
      }
      if (result.data === null) {
        return;
      }
      dispatch({ type: "panel", panel: result.data.panel });
      dispatch({ type: "drafts", drafts: result.data.drafts });
    });
  }, []);

  /** Leaves the reviewer's draft and the finding both exactly as they were. */
  const keepFinding = useCallback(() => dispatch({ type: "findingConflict", conflict: null }), []);

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
      if (event.metaKey && event.shiftKey && key === "f") {
        event.preventDefault();
        selectNextFinding();
        return;
      }
      if (event.metaKey && event.shiftKey && key === "y") {
        event.preventDefault();
        const id = selectedFindingId(current.panel);
        if (id !== null) {
          acceptFinding(id);
        }
        return;
      }
      if (event.metaKey && event.shiftKey && key === "d") {
        event.preventDefault();
        const id = selectedFindingId(current.panel);
        if (id !== null) {
          dismissFinding(id);
        }
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
  }, [
    state.status,
    isShowing,
    selectFile,
    toggleViewed,
    runReview,
    selectNextFinding,
    acceptFinding,
    dismissFinding,
  ]);

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
    revealFinding,
    acceptFinding,
    dismissFinding,
    replaceFinding,
    keepFinding,
  };
}
