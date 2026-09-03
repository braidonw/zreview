/** How long ago a moment was, in whole units, never less than none. */
function elapsed(atMs: number, nowMs: number) {
  // A clock that jumped backwards would otherwise read as a time in the future.
  const sinceMs = Math.max(nowMs - atMs, 0);
  return {
    minutes: Math.floor(sinceMs / 60_000),
    hours: Math.floor(sinceMs / 3_600_000),
    days: Math.floor(sinceMs / 86_400_000),
  };
}

/** A row's age, in the one unit that fits, as the ledger shows it. */
export function shortAge(updatedAtMs: number, nowMs: number): string {
  const { minutes, hours, days } = elapsed(updatedAtMs, nowMs);
  if (minutes < 60) {
    return `${minutes}m`;
  }
  if (hours < 24) {
    return `${hours}h`;
  }
  return `${days}d`;
}

/** The header stamp for a refresh that settled, in words. */
export function refreshedStamp(atMs: number, nowMs: number): string {
  const { minutes, hours, days } = elapsed(atMs, nowMs);
  if (minutes < 1) {
    return "Refreshed just now";
  }
  if (minutes < 60) {
    return `Refreshed ${minutes} min ago`;
  }
  if (hours < 24) {
    return `Refreshed ${hours} ${hours === 1 ? "hour" : "hours"} ago`;
  }
  return `Refreshed ${days} ${days === 1 ? "day" : "days"} ago`;
}
