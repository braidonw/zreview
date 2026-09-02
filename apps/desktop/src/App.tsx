import { useEffect, useState } from "react";
import type { LaunchDto } from "./bindings";
import { commands } from "./bindings";
import { HomeScreen } from "./components/HomeScreen";
import { SessionApp } from "./SessionApp";

export default function App() {
  const [launch, setLaunch] = useState<LaunchDto | null>(null);

  useEffect(() => {
    void commands.describeLaunch().then(setLaunch);
  }, []);

  // Nothing is rendered until the answer arrives, so neither screen flashes
  // before the other.
  if (launch === null) {
    return null;
  }
  if (launch === "Home") {
    return <HomeScreen />;
  }
  return <SessionApp description={launch.Session.description} />;
}
