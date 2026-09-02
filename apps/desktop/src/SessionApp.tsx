import { FailureScreen } from "./components/FailureScreen";
import { LoadingScreen } from "./components/LoadingScreen";
import { SessionShell } from "./components/SessionShell";
import { useSession } from "./hooks/useSession";

export function SessionApp({ description }: { description: string }) {
  const {
    state,
    selectFile,
    clickRow,
    openComposer,
    closeComposer,
    composerChange,
    composerDiscard,
    reanchorDraft,
  } = useSession(description);

  switch (state.status) {
    case "loading":
      return <LoadingScreen description={state.description} stage={state.stage} />;
    case "failed":
      return <FailureScreen failure={state.failure} />;
    case "ready":
      return (
        <SessionShell
          state={state}
          onSelectFile={selectFile}
          onRowClick={clickRow}
          onOpenComposer={openComposer}
          onComposerChange={composerChange}
          onComposerClose={closeComposer}
          onComposerDiscard={composerDiscard}
          onReanchorDraft={reanchorDraft}
        />
      );
  }
}
