import { useCallback, useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import type { HomeSnapshotDto } from "../bindings";
import { commands } from "../bindings";

/** Reads Home's repositories and exposes every action the screen can take on them. */
export function useHome() {
  const [snapshot, setSnapshot] = useState<HomeSnapshotDto | null>(null);
  const hasOpened = useRef(false);

  const refresh = useCallback(() => {
    void commands.refreshHome().then(setSnapshot);
  }, []);

  useEffect(() => {
    // Guarded so React StrictMode's double mount effect does not read the
    // settings file twice on the way in.
    if (hasOpened.current) {
      return;
    }
    hasOpened.current = true;
    refresh();
  }, [refresh]);

  const toggleFooter = useCallback(() => {
    void commands.toggleRepositoriesFooter().then(setSnapshot);
  }, []);

  const dismissRefusals = useCallback(() => {
    void commands.dismissRefusals().then(setSnapshot);
  }, []);

  const removeRepository = useCallback((path: string) => {
    void commands.removeRepository(path).then(setSnapshot);
  }, []);

  /** Opens the native folder picker, then hands what was chosen to the add command. */
  const addRepositories = useCallback(() => {
    void open({
      directory: true,
      multiple: true,
      title: "Add repositories",
    }).then((picked) => {
      if (picked === null) {
        return;
      }
      const folders = Array.isArray(picked) ? picked : [picked];
      return commands.addRepositories(folders).then(setSnapshot);
    });
  }, []);

  // r refreshes and e opens or closes the Repositories footer, as the layout
  // prototype bound them.
  useEffect(() => {
    function handleKeydown(event: KeyboardEvent) {
      if (event.metaKey || event.ctrlKey || event.altKey) {
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

  return { snapshot, refresh, toggleFooter, dismissRefusals, addRepositories, removeRepository };
}
