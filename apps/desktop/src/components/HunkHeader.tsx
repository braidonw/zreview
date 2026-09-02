import "./HunkHeader.css";

export function HunkHeader({ header }: { header: string }) {
  return <div className="hunk-header">{header}</div>;
}
