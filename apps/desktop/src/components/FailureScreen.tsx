import type { SessionFailureDto } from "../bindings";
import "./FailureScreen.css";

export function FailureScreen({ failure }: { failure: SessionFailureDto }) {
  return (
    <div className="failure-screen">
      <div className="failure-screen__summary">{failure.summary}</div>
      {failure.remediation !== null && (
        <div className="failure-screen__remediation">{failure.remediation}</div>
      )}
      {failure.detail !== null && <div className="failure-screen__detail">{failure.detail}</div>}
    </div>
  );
}
