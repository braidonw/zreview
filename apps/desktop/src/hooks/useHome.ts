import { useCallback, useEffect, useRef, useState } from "react";
import { Channel } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import type { CursorMoveDto, HomeSnapshotDto, SessionFailureDto } from "../bindings";
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

/** Reads Home's pull requests and exposes every action the screen can take on them. */
export function useHome() {
  const [snapshot, setSnapshot] = useState<HomeSnapshotDto | null>(null);
  const [failure, setFailure] = useState<SessionFailureDto | null>(null);
  const hasOpened = useRef(false);
  const refreshing = useRef(false);

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

  useEffect(() => {
    // Guarded so StrictMode's double mount effect does not refresh twice.
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

  return { snapshot, failure, toggleFooter, addRepositories, removeRepository };
}
