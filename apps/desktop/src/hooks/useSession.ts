import { useCallback, useEffect, useReducer, useRef } from "react";
import { Channel } from "@tauri-apps/api/core";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import type {
  DiffSideDto,
  FindingLocationDto,
  ReviewEventDto,
  ReviewPanelDto,
  SubmissionDto,
} from "../bindings";
import { commands } from "../bindings";
import { clamp } from "../lib/clamp";
import { toFailure } from "../lib/failure";
import { initialState, selectedFinding, selectionRange, sessionReducer } from "./sessionReducer";
import { useDraftQueue } from "./useDraftQueue";

/** What a command answering with `T`, or `null` when nothing can be reviewed. */
type CommandResult<T> = { status: "ok"; data: T | null } | { status: "error"; error: unknown };

/** What a command answering with the review panel hands back. */
type PanelResult = CommandResult<ReviewPanelDto>;

/** What a command answering with the submission hands back. */
type SubmissionResult =
  | { status: "ok"; data: SubmissionDto }
  | { status: "error"; error: unknown };

/**
 * Loads one session and exposes every action the UI can take on it.
 *
 * `isShowing` is false while Home is in front of this session, which keeps its
 * hidden tree from answering keystrokes meant for the list. `onRunningChange`
 * is told whenever the review run's state changes, whether or not this session
 * is showing, which is how Home's header slot follows a run it cannot see.
 */
export function useSession(
  description: string,
  isShowing: boolean,
  onRunningChange: (running: boolean) => void,
) {
  const [state, dispatch] = useReducer(sessionReducer, initialState);
  const stateRef = useRef(state);
  stateRef.current = state;
  const hasOpened = useRef(false);
  const draftQueue = useDraftQueue(dispatch);
  // Held here rather than read off the panel, because a second trigger can arrive
  // before the first run's own "Running" panel has come back over the channel.
  const isRunning = useRef(false);
  // The same, for the send. A double press must not reach the backend twice,
  // even though the backend refuses the second one anyway.
  const isSending = useRef(false);

  const isReviewRunning = state.status === "ready" && state.panel?.run.state === "Running";
  useEffect(() => {
    onRunningChange(isReviewRunning);
  }, [isReviewRunning, onRunningChange]);

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
   * Applies a finding command's answer: a refusal becomes a panel notice, and
   * an answer of `null` (nothing to review) is silently skipped. Only a real
   * answer reaches `onData`, which the caller uses to update the rest of the
   * state.
   */
  const applyFindingResult = useCallback(
    <T,>(promise: Promise<CommandResult<T>>, onData: (data: T) => void) => {
      promise.then((result) => {
        if (result.status === "error") {
          dispatch({ type: "panelNotice", notice: toFailure(result.error).summary });
          return;
        }
        if (result.data === null) {
          return;
        }
        onData(result.data);
      });
    },
    [],
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
      applyFindingResult(commands.revealFinding(id), (data) => {
        dispatch({ type: "panel", panel: data.panel });
        applyLocation(data.location);
      });
    },
    [applyFindingResult, applyLocation],
  );

  /** Moves the selection to the finding after the one selected, wrapping to the first. */
  const selectNextFinding = useCallback(() => {
    applyFindingResult(commands.selectNextFinding(), (data) => {
      dispatch({ type: "panel", panel: data.panel });
      applyLocation(data.location);
    });
  }, [applyFindingResult, applyLocation]);

  /**
   * Accepts a finding, turning it into a draft where the model allows it.
   *
   * When the anchor already holds the reviewer's own draft, neither text is
   * written; the panel asks whether to replace it instead, first revealing the
   * row so the reviewer can see what they are being asked about.
   */
  const acceptFinding = useCallback(
    (id: number) => {
      applyFindingResult(commands.acceptFinding(id), (data) => {
        const { panel, drafts, disposition } = data;
        dispatch({ type: "panel", panel });
        dispatch({ type: "drafts", drafts });
        if (disposition.outcome === "Occupied") {
          applyLocation(disposition.location);
          dispatch({
            type: "findingConflict",
            conflict: { id, existing: disposition.existing, proposed: disposition.proposed },
          });
        }
        // A finding about the change as a whole has nowhere to anchor, so its
        // proposal goes into the summary editor for the reviewer to edit or clear.
        if (disposition.outcome === "Summary") {
          dispatch({ type: "summary", body: disposition.body });
        }
      });
    },
    [applyFindingResult, applyLocation],
  );

  const dismissFinding = useCallback(
    (id: number) => {
      dispatch({ type: "findingConflict", conflict: null });
      onPanel(commands.dismissFinding(id));
    },
    [onPanel],
  );

  /**
   * Forces a finding onto its anchor, overwriting the reviewer's own draft
   * there. A stale answer (the finding this was asked about no longer exists,
   * or is no longer the one awaiting a reply) is refused rather than applied,
   * which is surfaced as a panel notice rather than treated as success.
   */
  const replaceFinding = useCallback(
    (id: number) => {
      dispatch({ type: "findingConflict", conflict: null });
      applyFindingResult(commands.overwriteFinding(id), (data) => {
        dispatch({ type: "panel", panel: data.panel });
        dispatch({ type: "drafts", drafts: data.drafts });
        if (data.disposition.outcome !== "Drafted") {
          dispatch({
            type: "panelNotice",
            notice: "This finding is no longer waiting to be replaced.",
          });
        }
      });
    },
    [applyFindingResult],
  );

  /** Leaves the reviewer's draft and the finding both exactly as they were. */
  const keepFinding = useCallback(() => dispatch({ type: "findingConflict", conflict: null }), []);

  /** Persists the summary, on the same queue the drafts use, on every keystroke. */
  const summaryChange = useCallback(
    (body: string) => draftQueue.editSummary(body),
    [draftQueue],
  );

  /**
   * Runs a submission command, showing a refusal in the panel.
   *
   * A command that never answered is about the review, not the sitting.
   * Replacing the Session with a failure screen would throw away the diff and
   * every draft over it.
   */
  const onSubmission = useCallback((call: Promise<SubmissionResult>) => {
    call
      .then((answered) => {
        if (answered.status === "error") {
          dispatch({ type: "panelNotice", notice: toFailure(answered.error).summary });
          return;
        }
        dispatch({ type: "submission", submission: answered.data });
      })
      .catch((error: unknown) => {
        dispatch({ type: "panelNotice", notice: toFailure(error).summary });
      });
  }, []);

  /**
   * Assembles what submitting would post and opens the confirmation.
   *
   * Posts nothing. What comes back is the exact request, which the reviewer
   * approves before anything leaves the machine.
   */
  const submit = useCallback(
    (event: ReviewEventDto) => onSubmission(commands.requestSubmission(event)),
    [onSubmission],
  );

  /** Puts the confirmation away, leaving every draft and the summary as they were. */
  const cancelSubmission = useCallback(
    () => onSubmission(commands.cancelSubmission()),
    [onSubmission],
  );

  /**
   * Posts the confirmed review.
   *
   * The backend refuses a second send while one is in flight, and the panel is
   * put into its sending state first so the confirmation's own buttons go with
   * it. A failure keeps every draft and the summary exactly where they were.
   */
  const sendSubmission = useCallback(() => {
    if (isSending.current) {
      return;
    }
    isSending.current = true;

    try {
      const channel = new Channel<SubmissionDto>();
      channel.onmessage = (submission) => dispatch({ type: "submission", submission });

      commands
        .sendSubmission(channel)
        .then((sent) => {
          if (sent.status === "error") {
            dispatch({ type: "panelNotice", notice: toFailure(sent.error).summary });
            return;
          }
          dispatch({ type: "submission", submission: sent.data.submission });
          dispatch({ type: "drafts", drafts: sent.data.drafts });
          // Only now is it safe for the editor to forget what was posted.
          dispatch({ type: "summary", body: sent.data.summary });
        })
        .catch((error: unknown) => {
          dispatch({ type: "panelNotice", notice: toFailure(error).summary });
        })
        .finally(() => {
          isSending.current = false;
        });
    } catch (error: unknown) {
      // A throw before the promise exists would otherwise leave the flag set and
      // nothing sendable for the rest of the sitting.
      isSending.current = false;
      dispatch({ type: "panelNotice", notice: toFailure(error).summary });
    }
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
      // The composer and the summary are real text editors, so global shortcuts
      // yield to either of them entirely.
      if (
        event.target instanceof HTMLElement &&
        event.target.closest("[data-composer], [data-summary-editor]")
      ) {
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
        const finding = selectedFinding(current.panel);
        if (finding !== null) {
          acceptFinding(finding.id);
        }
        return;
      }
      if (event.metaKey && event.shiftKey && key === "d") {
        event.preventDefault();
        const finding = selectedFinding(current.panel);
        if (finding !== null) {
          dismissFinding(finding.id);
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
    summaryChange,
    submit,
    cancelSubmission,
    sendSubmission,
  };
}
