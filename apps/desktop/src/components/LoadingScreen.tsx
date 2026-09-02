import "./LoadingScreen.css";

export function LoadingScreen({ description, stage }: { description: string; stage: string }) {
  return (
    <div className="loading-screen">
      <div className="loading-screen__description">Opening {description}</div>
      <div className="loading-screen__stage">{stage}</div>
    </div>
  );
}
