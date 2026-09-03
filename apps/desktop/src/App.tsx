import { useCallback, useEffect, useState } from "react";
import type { OpenSessionDto, SessionFailureDto, WindowDto } from "./bindings";
import { commands } from "./bindings";
import { FailureScreen } from "./components/FailureScreen";
import { HomeScreen } from "./components/HomeScreen";
import { toFailure } from "./lib/failure";
import { SessionApp } from "./SessionApp";
import "./App.css";

/** What a navigation command answers with. */
type WindowResult =
  | { status: "ok"; data: WindowDto }
  | { status: "error"; error: SessionFailureDto };

/** Which screen is in front, and the Session the window holds either way. */
function screens(shown: WindowDto): { isShowingHome: boolean; session: OpenSessionDto | null } {
  if ("Home" in shown && shown.Home !== undefined) {
    return { isShowingHome: true, session: shown.Home.alive };
  }
  return { isShowingHome: false, session: shown.Session.session };
}

export default function App() {
  const [shown, setShown] = useState<WindowDto | null>(null);
  const [failure, setFailure] = useState<SessionFailureDto | null>(null);

  useEffect(() => {
    commands
      .describeWindow()
      .then(setShown)
      .catch((error: unknown) => setFailure(toFailure(error)));
  }, []);

  /** Takes what a navigation answered, so a refusal is shown rather than swallowed. */
  const navigate = useCallback((call: Promise<WindowResult>) => {
    call
      .then((result) => {
        if (result.status === "error") {
          setFailure(toFailure(result.error));
          return;
        }
        setShown(result.data);
      })
      .catch((error: unknown) => setFailure(toFailure(error)));
  }, []);

  const openRow = useCallback(
    (repository: string, number: number) => navigate(commands.openRow(repository, number)),
    [navigate],
  );
  const returnToSession = useCallback(
    () => navigate(commands.returnToSession()),
    [navigate],
  );
  const returnToHome = useCallback(() => navigate(commands.returnToHome()), [navigate]);

  const { isShowingHome, session } =
    shown === null ? { isShowingHome: true, session: null } : screens(shown);
  // Only a Session opened from a row has a Home behind it to go back to.
  const canGoBack = !isShowingHome && session !== null && session.row_identity !== null;

  useEffect(() => {
    if (!canGoBack) {
      return;
    }
    function handleKeydown(event: KeyboardEvent) {
      // Escape stays reserved for dismissing composers and panels, so back
      // never eats a dismissal.
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
      {isShowingHome && (
        <HomeScreen
          aliveIdentity={session?.row_identity ?? null}
          onOpenRow={openRow}
          onReturnToSession={returnToSession}
        />
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
          />
        </div>
      )}
    </>
  );
}
