import { useEffect, useState } from "react";
import type { LaunchDto, SessionFailureDto } from "./bindings";
import { commands } from "./bindings";
import { FailureScreen } from "./components/FailureScreen";
import { HomeScreen } from "./components/HomeScreen";
import { toFailure } from "./lib/failure";
import { SessionApp } from "./SessionApp";

export default function App() {
  const [launch, setLaunch] = useState<LaunchDto | null>(null);
  const [failure, setFailure] = useState<SessionFailureDto | null>(null);

  useEffect(() => {
    commands
      .describeLaunch()
      .then(setLaunch)
      .catch((error: unknown) => setFailure(toFailure(error)));
  }, []);

  // A launch that never answers would otherwise leave the window blank.
  if (failure !== null) {
    return <FailureScreen failure={failure} />;
  }
  // Nothing is rendered until the answer arrives, so neither screen flashes first.
  if (launch === null) {
    return null;
  }
  if (launch === "Home") {
    return <HomeScreen />;
  }
  return <SessionApp description={launch.Session.description} />;
}
