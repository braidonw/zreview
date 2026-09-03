import { useCallback, useEffect, useRef, useState } from "react";
import { Channel } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type { CursorMoveDto, HomeRowDto, HomeSnapshotDto, SessionFailureDto } from "../bindings";
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

/**
 * Reads Home's pull requests and exposes every action the screen can take.
 *
 * Home stays mounted behind a Session, so `isShowing` is what decides whether
 * it answers the keyboard, listens for focus, and refreshes.
 */
export function useHome({
  isShowing,
  onOpenRow,
  onReturnToSession,
}: {
  isShowing: boolean;
  onOpenRow: (repository: string, number: number) => Promise<SessionFailureDto | null>;
  onReturnToSession: () => Promise<SessionFailureDto | null>;
}) {
  const [snapshot, setSnapshot] = useState<HomeSnapshotDto | null>(null);
  const [failure, setFailure] = useState<SessionFailureDto | null>(null);
  const [openFailure, setOpenFailure] = useState<SessionFailureDto | null>(null);
  const refreshing = useRef(false);
  // Read by the key handler, which is attached once and must still see the
  // list as it stands rather than as it was when it was attached.
  const shown = useRef<HomeSnapshotDto | null>(null);
  shown.current = snapshot;

  /** Takes what a command answered, so a rejection is shown rather than swallowed. */
  const apply = useCallback((call: Promise<HomeResult>) => {
    return call
      .then((result) => {
        if (result.status === "error") {
          setFailure(toFailure(result.error));
          return false;
        }
        setFailure(null);
        setSnapshot(result.data);
        return true;
      })
      .catch((error: unknown) => {
        setFailure(toFailure(error));
        return false;
      });
  }, []);

  /**
   * Refreshes, or leaves the running refresh alone.
   *
   * A trigger that arrives mid-refresh is ignored rather than queued, so the
   * list a reviewer is reading is never replaced by an older answer to a
   * question the running refresh is already asking.
   */
  const refresh = useCallback(() => {
    if (refreshing.current) {
      return;
    }
    refreshing.current = true;
    setOpenFailure(null);
    // Every batch carries what Home shows by then, so the list fills in as it loads.
    const channel = new Channel<HomeSnapshotDto>();
    channel.onmessage = (shown) => {
      setFailure(null);
      setSnapshot(shown);
    };
    apply(commands.refreshHome(channel)).finally(() => {
      refreshing.current = false;
    });
  }, [apply]);

  // Home refreshes when it appears, which is the first time it is drawn and
  // every return from a Session. What it last showed is what decides, so
  // StrictMode's double mount effect refreshes once and a refresh already
  // running is left to settle the screen itself.
  const showed = useRef(false);
  useEffect(() => {
    if (showed.current === isShowing) {
      return;
    }
    showed.current = isShowing;
    if (isShowing) {
      refresh();
    }
  }, [isShowing, refresh]);

  // Coming back from the browser shows the current state. Nothing polls, so
  // this and `r` are all that ever ask GitHub anything while Home is up. The
  // listener goes with the screen, so a Session in front of Home refreshes
  // nothing.
  useEffect(() => {
    if (!isShowing) {
      return;
    }
    let listening = true;
    let stopListening: (() => void) | null = null;
    getCurrentWindow()
      .onFocusChanged(({ payload: focused }) => {
        if (focused) {
          refresh();
        }
      })
      .then((stop) => {
        if (listening) {
          stopListening = stop;
        } else {
          stop();
        }
      })
      .catch((error: unknown) => setFailure(toFailure(error)));
    return () => {
      listening = false;
      stopListening?.();
    };
  }, [isShowing, refresh]);

  const toggleFooter = useCallback(() => {
    commands
      .toggleRepositoriesFooter()
      .then((toggled) => {
        setFailure(null);
        setSnapshot(toggled);
      })
      .catch((error: unknown) => setFailure(toFailure(error)));
  }, []);

  /** Runs `action`, then refreshes, so what the settings file now lists is listed. */
  const thenRefresh = useCallback(
    (action: Promise<HomeResult>) => {
      apply(action).then((changed) => {
        if (changed) {
          refresh();
        }
      });
    },
    [apply, refresh],
  );

  const removeRepository = useCallback(
    (path: string) => {
      thenRefresh(commands.removeRepository(path));
    },
    [thenRefresh],
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
        thenRefresh(commands.addRepositories(folders));
      })
      .catch((error: unknown) => setFailure(toFailure(error)));
  }, [thenRefresh]);

  const moveCursor = useCallback((moveTo: CursorMoveDto) => {
    commands
      .moveHomeCursor(moveTo)
      .then((moved) => {
        setFailure(null);
        setSnapshot(moved);
      })
      .catch((error: unknown) => setFailure(toFailure(error)));
  }, []);

  /**
   * Opens `row`'s pull request, or returns to it when its Session is the one
   * alive behind Home.
   *
   * Returning never reloads, which is what keeps a half-finished review whole.
   * A refusal stays on Home, which is where the reviewer can act on it.
   */
  const openRow = useCallback(
    (row: HomeRowDto) => {
      const navigated = row.is_alive ? onReturnToSession() : onOpenRow(row.repository, row.number);
      navigated.then(setOpenFailure);
    },
    [onOpenRow, onReturnToSession],
  );

  const openCursorRow = useCallback(() => {
    const listed = shown.current;
    if (listed === null) {
      return;
    }
    const row = listed.groups
      .flatMap((group) => group.rows)
      .find((candidate) => candidate.index === listed.cursor);
    if (row === undefined) {
      return;
    }
    openRow(row);
  }, [openRow]);

  // r refreshes, e opens or closes the Repositories footer, j and k walk the
  // rows, and Enter opens the one under the cursor.
  useEffect(() => {
    if (!isShowing) {
      return;
    }
    function handleKeydown(event: KeyboardEvent) {
      if (event.metaKey || event.ctrlKey || event.altKey || event.shiftKey) {
        return;
      }
      if (isTyping(event.target)) {
        return;
      }
      const key = event.key.toLowerCase();
      if (key === "enter") {
        event.preventDefault();
        openCursorRow();
        return;
      }
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
  }, [isShowing, moveCursor, openCursorRow, refresh, toggleFooter]);

  return {
    snapshot,
    failure,
    openFailure,
    toggleFooter,
    addRepositories,
    removeRepository,
    openRow,
  };
}
