import { useCallback, useEffect, useRef, useState } from "react";
import { Channel } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import type { CursorMoveDto, HomeSnapshotDto, RefreshStateDto, SessionFailureDto } from "../bindings";
import { commands } from "../bindings";
import { toFailure } from "../lib/failure";

/** What a command that touches the settings file answers with. */
type HomeResult =
  | { status: "ok"; data: HomeSnapshotDto }
  | { status: "error"; error: SessionFailureDto };

/** Whether a keystroke belongs to something the reviewer is typing into. */
function isTyping(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) {
    return false;
  }
  return target.isContentEditable || target.closest("input, textarea") !== null;
}

/** Reads Home's repositories and exposes every action the screen can take on them. */
export function useHome() {
  const [snapshot, setSnapshot] = useState<HomeSnapshotDto | null>(null);
  const [failure, setFailure] = useState<SessionFailureDto | null>(null);
  const hasOpened = useRef(false);

  /** Takes what a command answered, so a rejection is shown rather than swallowed. */
  const apply = useCallback((call: Promise<HomeResult>) => {
    call
      .then((result) => {
        if (result.status === "error") {
          setFailure(toFailure(result.error));
          return;
        }
        setFailure(null);
        setSnapshot(result.data);
      })
      .catch((error: unknown) => setFailure(toFailure(error)));
  }, []);

  /** What the running refresh last reported, which outranks the snapshot's own stamp. */
  const [progress, setProgress] = useState<RefreshStateDto | null>(null);

  const refresh = useCallback(() => {
    const channel = new Channel<RefreshStateDto>();
    channel.onmessage = setProgress;
    // The answer carries the settled stamp, so the reported one has done its job.
    apply(commands.refreshHome(channel).finally(() => setProgress(null)));
  }, [apply]);

  useEffect(() => {
    // Guarded so StrictMode's double mount effect does not read the file twice.
    if (hasOpened.current) {
      return;
    }
    hasOpened.current = true;
    refresh();
  }, [refresh]);

  const toggleFooter = useCallback(() => {
    commands
      .toggleRepositoriesFooter()
      .then((toggled) => {
        setFailure(null);
        setSnapshot(toggled);
      })
      .catch((error: unknown) => setFailure(toFailure(error)));
  }, []);

  const removeRepository = useCallback(
    (path: string) => {
      apply(commands.removeRepository(path));
    },
    [apply],
  );

  /** Opens the native folder picker, then hands what was chosen to the add command. */
  const addRepositories = useCallback(() => {
    open({
      directory: true,
      multiple: true,
      title: "Add repositories",
    })
      .then((picked) => {
        if (picked === null) {
          return;
        }
        const folders = Array.isArray(picked) ? picked : [picked];
        apply(commands.addRepositories(folders));
      })
      .catch((error: unknown) => setFailure(toFailure(error)));
  }, [apply]);

  const moveCursor = useCallback((moveTo: CursorMoveDto) => {
    commands
      .moveHomeCursor(moveTo)
      .then((moved) => {
        setFailure(null);
        setSnapshot(moved);
      })
      .catch((error: unknown) => setFailure(toFailure(error)));
  }, []);

  // r refreshes, e opens or closes the Repositories footer, and j and k walk the
  // rows, as the layout prototype bound them.
  useEffect(() => {
    function handleKeydown(event: KeyboardEvent) {
      if (event.metaKey || event.ctrlKey || event.altKey || event.shiftKey) {
        return;
      }
      if (isTyping(event.target)) {
        return;
      }
      const key = event.key.toLowerCase();
      if (key === "r") {
        event.preventDefault();
        refresh();
        return;
      }
      if (key === "e") {
        event.preventDefault();
        toggleFooter();
        return;
      }
      if (key === "j") {
        event.preventDefault();
        moveCursor("Down");
        return;
      }
      if (key === "k") {
        event.preventDefault();
        moveCursor("Up");
      }
    }

    window.addEventListener("keydown", handleKeydown);
    return () => window.removeEventListener("keydown", handleKeydown);
  }, [moveCursor, refresh, toggleFooter]);

  // What the running refresh reports outranks the stamp the last one left.
  const refreshState = progress ?? snapshot?.refresh ?? null;

  return {
    snapshot,
    refreshState,
    failure,
    refresh,
    toggleFooter,
    addRepositories,
    removeRepository,
  };
}
