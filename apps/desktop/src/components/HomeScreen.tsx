import type { HomeSnapshotDto } from "../bindings";
import { useHome } from "../hooks/useHome";
import { FailureScreen } from "./FailureScreen";
import "./HomeScreen.css";

/** The screen ZReview opens on when no pull request has been named. */
export function HomeScreen() {
  const { snapshot, toggleFooter, dismissRefusals, addRepositories, removeRepository } = useHome();

  // Nothing is drawn until the first read answers, so no empty state flashes
  // in front of a list that is about to arrive.
  if (snapshot === null) {
    return null;
  }

  return (
    <div className="home">
      <header className="home__header">
        <div className="home__heading">
          <h1 className="home__title">Home</h1>
          {snapshot.count_line !== null && (
            <span className="home__count">{snapshot.count_line}</span>
          )}
        </div>
      </header>
      <HomeBody snapshot={snapshot} onAdd={addRepositories} />
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
        {snapshot.refusals.length > 0 && (
          <div className="home__refusals">
            {snapshot.refusals.map((refusal) => (
              <div className="home__refusal" key={refusal.folder}>
                {refusal.folder}: {refusal.reason}
              </div>
            ))}
            <button className="home__dismiss" type="button" onClick={dismissRefusals}>
              Dismiss
            </button>
          </div>
        )}
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

/** The list area, which the empty state and a whole-Home failure each replace. */
function HomeBody({ snapshot, onAdd }: { snapshot: HomeSnapshotDto; onAdd: () => void }) {
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
      {snapshot.groups.map((group) => (
        <section className="home__group" key={group.title}>
          <div className="home__group-label">
            <span className="home__label">{group.title}</span>
            <span className="home__group-count">{group.count}</span>
          </div>
          {group.count === 0 && <div className="home__group-empty">{group.empty_copy}</div>}
        </section>
      ))}
    </div>
  );
}
