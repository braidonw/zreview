import { useCallback, useEffect, useReducer, useRef } from "react";
import { Channel } from "@tauri-apps/api/core";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import type { SessionFailureDto } from "../bindings";
import { commands } from "../bindings";
import { clamp } from "../lib/clamp";
import { initialState, selectionRange, sessionReducer } from "./sessionReducer";

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

/** Loads the demo session and exposes every action the UI can take on it. */
export function useSession() {
  const [state, dispatch] = useReducer(sessionReducer, initialState);
  const stateRef = useRef(state);
  stateRef.current = state;
  const hasOpened = useRef(false);

  useEffect(() => {
    if (hasOpened.current) {
      return;
    }
    hasOpened.current = true;

    const channel = new Channel<string>();
    channel.onmessage = (stage) => dispatch({ type: "stage", stage });

    commands.openSession(channel).then((opened) => {
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
    });
  }, []);

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

  useEffect(() => {
    if (state.status !== "ready") {
      return;
    }

    function handleKeydown(event: KeyboardEvent) {
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

  return { state, selectFile, clickRow };
}
