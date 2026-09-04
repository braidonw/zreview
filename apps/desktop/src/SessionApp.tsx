import { FailureScreen } from "./components/FailureScreen";
import { LoadingScreen } from "./components/LoadingScreen";
import { SessionShell } from "./components/SessionShell";
import { useSession } from "./hooks/useSession";

export function SessionApp({
  description,
  isShowing,
  onBack,
}: {
  description: string;
  /** False while Home is in front, which is when this Session yields the keyboard. */
  isShowing: boolean;
  /** Absent for a Session with no Home behind it, which offers no way back. */
  onBack: (() => void) | null;
}) {
  const {
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
  } = useSession(description, isShowing);

  switch (state.status) {
    case "loading":
      return <LoadingScreen description={state.description} stage={state.stage} />;
    case "failed":
      return <FailureScreen failure={state.failure} />;
    case "ready":
      return (
        <SessionShell
          state={state}
          isShowing={isShowing}
          onBack={onBack}
          onSelectFile={selectFile}
          onRowClick={clickRow}
          onOpenComposer={openComposer}
          onComposerChange={composerChange}
          onComposerClose={closeComposer}
          onComposerDiscard={composerDiscard}
          onReanchorDraft={reanchorDraft}
          onRunReview={runReview}
          onCancelReview={cancelReview}
          onToggleGuidanceSection={toggleGuidanceSection}
          onToggleGuidanceFile={toggleGuidanceFile}
          onRevealFinding={revealFinding}
          onAcceptFinding={acceptFinding}
          onDismissFinding={dismissFinding}
          onReplaceFinding={replaceFinding}
          onKeepFinding={keepFinding}
        />
      );
  }
}
