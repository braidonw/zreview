import { useEffect, useRef } from "react";
import type { HomeRowDto, HomeSnapshotDto, RefreshStateDto, SessionFailureDto } from "../bindings";
import type { OpenRowResult } from "../hooks/useHome";
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
export function HomeScreen({
  isShowing,
  aliveIdentity,
  isReviewRunning,
  onOpenRow,
  onCancelRunAndOpenRow,
  onReturnToSession,
}: {
  /** False while a Session is in front, which is when Home goes quiet. */
  isShowing: boolean;
  /** `owner/name#number` of the Session alive behind Home, if one is. */
  aliveIdentity: string | null;
  /** Whether that Session has a review run in flight. */
  isReviewRunning: boolean;
  onOpenRow: (repository: string, number: number) => Promise<OpenRowResult>;
  onCancelRunAndOpenRow: (repository: string, number: number) => Promise<SessionFailureDto | null>;
  onReturnToSession: () => Promise<SessionFailureDto | null>;
}) {
  const {
    snapshot,
    failure,
    openFailure,
    toggleFooter,
    addRepositories,
    removeRepository,
    openRow,
    runConfirmation,
    cancelRunAndOpenRow,
    stayOnHome,
  } = useHome({ isShowing, onOpenRow, onCancelRunAndOpenRow, onReturnToSession });
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
      <Header
        countLine={snapshot.count_line}
        stamp={stamp}
        aliveIdentity={aliveIdentity}
        isReviewRunning={isReviewRunning}
        onReturnToSession={onReturnToSession}
      />
      {runConfirmation !== null && (
        <RunConfirmation
          row={runConfirmation}
          onCancelAndContinue={cancelRunAndOpenRow}
          onStay={stayOnHome}
        />
      )}
      <HomeBody
        snapshot={snapshot}
        openFailure={openFailure}
        nowMs={nowMs}
        onAdd={addRepositories}
        onRemove={removeRepository}
        onOpenRow={openRow}
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

function Header({
  countLine,
  stamp,
  aliveIdentity,
  isReviewRunning,
  onReturnToSession,
}: {
  countLine: string | null;
  stamp: string | null;
  aliveIdentity: string | null;
  isReviewRunning: boolean;
  onReturnToSession: () => void;
}) {
  return (
    <header className="home__header">
      <div className="home__heading">
        <h1 className="home__title">Home</h1>
        {countLine !== null && <span className="home__count">{countLine}</span>}
      </div>
      <div className="home__header-right">
        {/* The way back to a Session whose pull request has no row at all. */}
        {aliveIdentity !== null && (
          <button className="home__slot" type="button" onClick={onReturnToSession}>
            <span className="home__slot-identity">{aliveIdentity}</span>
            {isReviewRunning && <span className="home__slot-status">review running</span>}
            <span className="home__keycap">cmd-[</span>
          </button>
        )}
        {stamp !== null && (
          <div className="home__stamp">
            <span className="home__stamp-text">{stamp}</span>
            <span className="home__keycap">r</span>
          </div>
        )}
      </div>
    </header>
  );
}

/**
 * The confirmation opening a different row asks for while the Session behind
 * Home has a live run in the way. Exactly two choices, no third way out.
 */
function RunConfirmation({
  row,
  onCancelAndContinue,
  onStay,
}: {
  row: HomeRowDto;
  onCancelAndContinue: () => void;
  onStay: () => void;
}) {
  return (
    <div className="home__confirm" role="alertdialog" aria-label="A review is running">
      <span className="home__confirm-text">
        The Session behind Home is still reviewing. Cancel it and open {row.identity}?
      </span>
      <div className="home__confirm-actions">
        <button className="home__button" type="button" onClick={onStay}>
          Stay
        </button>
        <button
          className="home__button home__button--primary"
          type="button"
          onClick={onCancelAndContinue}
        >
          Cancel run and continue
        </button>
      </div>
    </div>
  );
}

/** The list area, which the empty state and a whole-Home failure each replace. */
function HomeBody({
  snapshot,
  openFailure,
  nowMs,
  onAdd,
  onRemove,
  onOpenRow,
}: {
  snapshot: HomeSnapshotDto;
  /** Why the last row would not open, until the next refresh asks again. */
  openFailure: SessionFailureDto | null;
  nowMs: number;
  onAdd: () => void;
  onRemove: (path: string) => void;
  onOpenRow: (row: HomeRowDto) => void;
}) {
  const blocked = snapshot.failure ?? openFailure;
  if (blocked !== null) {
    return (
      <div className="home__body">
        <FailureScreen failure={blocked} />
      </div>
    );
  }
  if (snapshot.repositories.length === 0) {
    return (
      <div className="home__body home__body--empty">
        <FailureLine failure={snapshot.write_failure} />
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
      <FailureLine failure={snapshot.write_failure} />
      <FailureLine failure={snapshot.drafts_failure} />
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
                <Row
                  key={row.identity}
                  row={row}
                  cursor={row.index === snapshot.cursor}
                  alive={row.is_alive}
                  nowMs={nowMs}
                  onOpen={onOpenRow}
                />
              ))}
            </ul>
          )}
        </section>
      ))}
    </div>
  );
}

/**
 * One failure as a line above the list: summary, then remediation, then
 * detail, whichever of the two are present. Shared by the settings write
 * failure and the Drafts read failure, which sit in the same place and read
 * the same way.
 */
function FailureLine({ failure }: { failure: SessionFailureDto | null }) {
  if (failure === null) {
    return null;
  }
  return (
    <div className="home__failure-line">
      <span>{failure.summary}</span>
      {failure.remediation !== null && (
        <span className="home__failure-detail">{failure.remediation}</span>
      )}
      {failure.detail !== null && <span className="home__failure-detail">{failure.detail}</span>}
    </div>
  );
}

/** One pull request on one line, its columns aligned whether or not they speak. */
function Row({
  row,
  cursor,
  alive,
  nowMs,
  onOpen,
}: {
  row: HomeRowDto;
  cursor: boolean;
  alive: boolean;
  nowMs: number;
  onOpen: (row: HomeRowDto) => void;
}) {
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
      onClick={() => onOpen(row)}
    >
      <span className="home__row-title">{row.title}</span>
      <span className="home__cell home__cell--drafts">
        {row.drafts_label !== null && (
          <span className="home__drafts-badge">{row.drafts_label}</span>
        )}
      </span>
      <span className="home__cell home__cell--review">
        {alive && <span className="home__alive-mark" aria-hidden="true" />}
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
