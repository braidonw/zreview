import { useCallback, useEffect, useState } from "react";
import type { OpenSessionDto, SessionFailureDto, WindowDto } from "./bindings";
import { commands } from "./bindings";
import { FailureScreen } from "./components/FailureScreen";
import { HomeScreen } from "./components/HomeScreen";
import type { OpenRowResult } from "./hooks/useHome";
import { toFailure } from "./lib/failure";
import { SessionApp } from "./SessionApp";
import "./App.css";

/** What a navigation command answers with. */
type WindowResult =
  | { status: "ok"; data: WindowDto }
  | { status: "error"; error: SessionFailureDto };

/** Which screen the window shows, and the Session it holds either way. */
type Screen = { isShowingHome: boolean; session: OpenSessionDto | null };

function currentScreen(shown: WindowDto): Screen {
  if ("Home" in shown && shown.Home !== undefined) {
    return { isShowingHome: true, session: shown.Home.alive };
  }
  if ("Session" in shown && shown.Session !== undefined) {
    return { isShowingHome: false, session: shown.Session.session };
  }
  // A window the backend grew a third screen for, which nothing here can draw.
  const unknown: never = shown;
  throw new Error(`the window is showing something unknown ${JSON.stringify(unknown)}`);
}

export default function App() {
  const [shown, setShown] = useState<WindowDto | null>(null);
  const [failure, setFailure] = useState<SessionFailureDto | null>(null);
  // Told by the Session behind Home whenever its run's state changes, so the
  // header slot follows a run it cannot see. Reset by the fresh `useSession`
  // a new Session mounts with, whether by a row opening one or replacing one.
  const [isReviewRunning, setIsReviewRunning] = useState(false);

  useEffect(() => {
    commands
      .describeWindow()
      .then(setShown)
      .catch((error: unknown) => setFailure(toFailure(error)));
  }, []);

  /**
   * Navigates, answering with the refusal when the window would not.
   *
   * A refusal is handed back rather than shown here, because the screen that
   * asked is still on and is where the reviewer can act on it.
   */
  const navigate = useCallback((call: Promise<WindowResult>) => {
    return call
      .then((result) => {
        if (result.status === "error") {
          return toFailure(result.error);
        }
        setShown(result.data);
        return null;
      })
      .catch((error: unknown) => toFailure(error));
  }, []);

  /**
   * Opens `repository#number`, answering with whether it opened, whether a
   * live run behind Home blocked it, or a refusal.
   *
   * A block is not a failure and is not shown here: Home renders the
   * confirmation and asks again through `cancelRunAndOpenRow`.
   */
  const openRow = useCallback((repository: string, number: number): Promise<OpenRowResult> => {
    return commands
      .openRow(repository, number)
      .then((result) => {
        if (result.status === "error") {
          return { status: "error" as const, error: toFailure(result.error) };
        }
        if (result.data.outcome === "Blocked") {
          return { status: "blocked" as const };
        }
        setShown(result.data.window);
        return { status: "opened" as const };
      })
      .catch((error: unknown) => ({ status: "error" as const, error: toFailure(error) }));
  }, []);
  /** Cancels the run in the way, then opens the row, once Home has confirmed. */
  const cancelRunAndOpenRow = useCallback(
    (repository: string, number: number) =>
      navigate(commands.openRowCancellingRun(repository, number)),
    [navigate],
  );
  const returnToSession = useCallback(() => navigate(commands.returnToSession()), [navigate]);
  const returnToHome = useCallback(() => {
    navigate(commands.returnToHome()).then((refused) => {
      if (refused !== null) {
        setFailure(refused);
      }
    });
  }, [navigate]);

  const { isShowingHome, session } =
    shown === null ? { isShowingHome: true, session: null } : currentScreen(shown);
  // Only a Session opened from a row has a Home behind it to go back to.
  const canGoBack = !isShowingHome && session !== null && session.row_identity !== null;
  // A window the command line opened straight into a Session has no Home at all.
  const hasHome = shown !== null && (isShowingHome || canGoBack);

  // Cmd-[ is the only binding that goes back. Escape stays reserved for
  // dismissing composers and panels, so back never eats a dismissal.
  useEffect(() => {
    if (!canGoBack) {
      return;
    }
    function handleKeydown(event: KeyboardEvent) {
      if (event.metaKey && event.key === "[") {
        event.preventDefault();
        returnToHome();
      }
    }
    window.addEventListener("keydown", handleKeydown);
    return () => window.removeEventListener("keydown", handleKeydown);
  }, [canGoBack, returnToHome]);

  // A window that never said what it holds would otherwise be blank for ever.
  if (failure !== null) {
    return <FailureScreen failure={failure} />;
  }
  // Nothing is rendered until the answer arrives, so neither screen flashes first.
  if (shown === null) {
    return null;
  }

  return (
    <>
      {hasHome && (
        // Kept mounted behind the Session, so a refresh still running when a
        // row was opened has somewhere to land and settles the screen on return.
        <div className="app__home" hidden={!isShowingHome}>
          <HomeScreen
            isShowing={isShowingHome}
            aliveIdentity={session?.row_identity ?? null}
            isReviewRunning={isReviewRunning}
            onOpenRow={openRow}
            onCancelRunAndOpenRow={cancelRunAndOpenRow}
            onReturnToSession={returnToSession}
          />
        </div>
      )}
      {session !== null && (
        // Kept mounted while Home shows, so the file, cursor, and unsent
        // composer text of the Session behind it survive exactly.
        <div className="app__session" hidden={isShowingHome}>
          <SessionApp
            key={session.row_identity ?? "the command line"}
            description={session.description}
            isShowing={!isShowingHome}
            onBack={canGoBack ? returnToHome : null}
            onRunningChange={setIsReviewRunning}
          />
        </div>
      )}
    </>
  );
}
