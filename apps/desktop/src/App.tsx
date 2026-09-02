import { FailureScreen } from "./components/FailureScreen";
import { LoadingScreen } from "./components/LoadingScreen";
import { SessionShell } from "./components/SessionShell";
import { useSession } from "./hooks/useSession";

export default function App() {
  const { state, selectFile, clickRow } = useSession();

  switch (state.status) {
    case "loading":
      return <LoadingScreen description={state.description} stage={state.stage} />;
    case "failed":
      return <FailureScreen failure={state.failure} />;
    case "ready":
      return <SessionShell state={state} onSelectFile={selectFile} onRowClick={clickRow} />;
  }
}
