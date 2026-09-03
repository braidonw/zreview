import { useEffect, useRef } from "react";
import type { HomeRowDto, HomeSnapshotDto, RefreshStateDto, SessionFailureDto } from "../bindings";
import { useHome } from "../hooks/useHome";
import { useNow } from "../hooks/useNow";
import { refreshedStamp, shortAge } from "../lib/relativeTime";
import { FailureScreen } from "./FailureScreen";
import "./HomeScreen.css";

/** What the header stamp reads, absent before the first refresh starts. */
function stampText(refresh: RefreshStateDto, nowMs: number): string | null {
  if (refresh === "NeverRefreshed") {
    return null;
  }
  if (refresh === "Failed") {
    return "Refresh failed";
  }
  if ("Refreshing" in refresh && refresh.Refreshing !== undefined) {
    const { done, total } = refresh.Refreshing;
    // The total is unknown until the settings file has been read.
    return total === 0 ? "Refreshing" : `Refreshing ${done} of ${total}`;
  }
  return refreshedStamp(refresh.Refreshed.at_ms, nowMs);
}

/** The screen ZReview opens on when no pull request has been named. */
export function HomeScreen() {
  const { snapshot, failure, toggleFooter, addRepositories, removeRepository } = useHome();
  const nowMs = useNow();

  // A command that never answered leaves nothing worth trusting on screen.
  if (failure !== null) {
    return <FailureScreen failure={failure} />;
  }
  // Nothing is drawn until the refresh says it has started, which it does before
  // it asks GitHub anything, so the window is blank for no longer than that.
  if (snapshot === null) {
    return null;
  }
  const stamp = stampText(snapshot.refresh, nowMs);

  return (
    <div className="home">
      <Header countLine={snapshot.count_line} stamp={stamp} />
      <HomeBody
        snapshot={snapshot}
        nowMs={nowMs}
        onAdd={addRepositories}
        onRemove={removeRepository}
      />
      <footer className="home__footer">
        <div className="home__footer-line">
          <button className="home__footer-toggle" type="button" onClick={toggleFooter}>
            <span className="home__label">Repositories</span>
            <span className="home__summary">{snapshot.footer_summary}</span>
          </button>
          <button className="home__button" type="button" onClick={addRepositories}>
            Add...
          </button>
        </div>
        {snapshot.refusals.map((refusal) => (
          <div className="home__refusal" key={refusal.path}>
            {refusal.path}: {refusal.reason}
          </div>
        ))}
        {snapshot.footer_expanded &&
          snapshot.repositories.map((repository) => (
            <div className="home__repository" key={repository.path}>
              <span className="home__slug">{repository.slug}</span>
              <span className="home__path">{repository.path}</span>
              <span className="home__state">{repository.failure}</span>
              <button
                className="home__remove"
                type="button"
                onClick={() => removeRepository(repository.path)}
              >
                Remove
              </button>
            </div>
          ))}
      </footer>
    </div>
  );
}

function Header({ countLine, stamp }: { countLine: string | null; stamp: string | null }) {
  return (
    <header className="home__header">
      <div className="home__heading">
        <h1 className="home__title">Home</h1>
        {countLine !== null && <span className="home__count">{countLine}</span>}
      </div>
      {stamp !== null && (
        <div className="home__stamp">
          <span className="home__stamp-text">{stamp}</span>
          <span className="home__keycap">r</span>
        </div>
      )}
    </header>
  );
}

/** The list area, which the empty state and a whole-Home failure each replace. */
function HomeBody({
  snapshot,
  nowMs,
  onAdd,
  onRemove,
}: {
  snapshot: HomeSnapshotDto;
  nowMs: number;
  onAdd: () => void;
  onRemove: (path: string) => void;
}) {
  if (snapshot.failure !== null) {
    return (
      <div className="home__body">
        <FailureScreen failure={snapshot.failure} />
      </div>
    );
  }
  if (snapshot.repositories.length === 0) {
    return (
      <div className="home__body home__body--empty">
        <WriteFailure failure={snapshot.write_failure} />
        <h2 className="home__empty-heading">No repositories yet</h2>
        <p className="home__empty-copy">
          Add a local clone and Home lists the pull requests that want you.
        </p>
        <button className="home__button home__button--primary" type="button" onClick={onAdd}>
          Add repository...
        </button>
      </div>
    );
  }
  return (
    <div className="home__body home__body--list">
      <WriteFailure failure={snapshot.write_failure} />
      <DraftsFailure failure={snapshot.drafts_failure} />
      {snapshot.failed_repositories.map((repository) => (
        <div className="home__failed-repository" key={repository.path}>
          <span className="home__failed-detail">
            {`${repository.slug} · ${repository.reason}`}
          </span>
          <button className="home__remove" type="button" onClick={() => onRemove(repository.path)}>
            Remove
          </button>
        </div>
      ))}
      {snapshot.groups.map((group) => (
        <section className="home__group" key={group.title}>
          <div className="home__group-label">
            <span className="home__label">{group.title}</span>
            <span className="home__group-count">{group.count}</span>
          </div>
          {group.count === 0 && <div className="home__group-empty">{group.empty_copy}</div>}
          {group.count > 0 && (
            <ul className="home__rows">
              {group.rows.map((row) => (
                <Row key={row.identity} row={row} cursor={row.index === snapshot.cursor} nowMs={nowMs} />
              ))}
            </ul>
          )}
        </section>
      ))}
    </div>
  );
}

/** Why the last write did not reach the settings file, wherever the list is. */
function WriteFailure({ failure }: { failure: SessionFailureDto | null }) {
  if (failure === null) {
    return null;
  }
  return (
    <div className="home__write-failure">
      <span>{failure.summary}</span>
      {failure.remediation !== null && (
        <span className="home__write-remediation">{failure.remediation}</span>
      )}
    </div>
  );
}

/** Why the last Drafts read failed, above the list beside the failed repositories. */
function DraftsFailure({ failure }: { failure: SessionFailureDto | null }) {
  if (failure === null) {
    return null;
  }
  return (
    <div className="home__drafts-failure">
      <span>{failure.summary}</span>
      {failure.detail !== null && <span className="home__drafts-failure-detail">{failure.detail}</span>}
    </div>
  );
}

/** One pull request on one line, its columns aligned whether or not they speak. */
function Row({ row, cursor, nowMs }: { row: HomeRowDto; cursor: boolean; nowMs: number }) {
  const line = useRef<HTMLLIElement>(null);

  useEffect(() => {
    if (cursor) {
      line.current?.scrollIntoView({ block: "nearest" });
    }
  }, [cursor]);

  return (
    <li
      ref={line}
      className={`home__row${cursor ? " home__row--cursor" : ""}`}
      aria-selected={cursor}
    >
      <span className="home__row-title">{row.title}</span>
      <span className="home__cell home__cell--drafts">
        {row.drafts !== null && <span className="home__drafts-badge">{row.drafts}</span>}
      </span>
      <span className="home__cell home__cell--review">
        {row.review_status !== null && (
          <span className={`home__status home__status--${row.review_status.tone.toLowerCase()}`}>
            {row.review_status.label}
          </span>
        )}
      </span>
      <span className="home__cell home__cell--checks">
        {row.check_status !== null && (
          <span className={`home__status home__status--${row.check_status.tone.toLowerCase()}`}>
            {row.check_status.label}
          </span>
        )}
      </span>
      <span className="home__cell home__cell--identity">{row.identity}</span>
      <span className="home__cell home__cell--author">{row.author}</span>
      <span className="home__cell home__cell--age">{shortAge(row.updated_at_ms, nowMs)}</span>
    </li>
  );
}
