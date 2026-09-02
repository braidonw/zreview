import { useCallback, useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import type { HomeSnapshotDto, SessionFailureDto } from "../bindings";
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

  const refresh = useCallback(() => {
    apply(commands.refreshHome());
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

  // r refreshes and e opens or closes the Repositories footer, as the layout prototype bound them.
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
      }
    }

    window.addEventListener("keydown", handleKeydown);
    return () => window.removeEventListener("keydown", handleKeydown);
  }, [refresh, toggleFooter]);

  return { snapshot, failure, refresh, toggleFooter, addRepositories, removeRepository };
}
