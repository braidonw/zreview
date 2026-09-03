import { describe, expect, it } from "vitest";
import { refreshedStamp, shortAge } from "./relativeTime";

const minute = 60_000;
const hour = 60 * minute;
const day = 24 * hour;
const now = 1_788_266_096_000;

describe("shortAge", () => {
  it("counts minutes, then hours, then days", () => {
    expect(shortAge(now - 40 * minute, now)).toBe("40m");
    expect(shortAge(now - 2 * hour, now)).toBe("2h");
    expect(shortAge(now - 1 * day, now)).toBe("1d");
    expect(shortAge(now - 30 * day, now)).toBe("30d");
  });

  it("rounds down to the unit it is showing", () => {
    expect(shortAge(now - (59 * minute + 59_000), now)).toBe("59m");
    expect(shortAge(now - (23 * hour + 59 * minute), now)).toBe("23h");
  });

  it("shows anything under a minute as no minutes at all", () => {
    expect(shortAge(now - 30_000, now)).toBe("0m");
    expect(shortAge(now, now)).toBe("0m");
  });

  // A clock that has jumped backwards must not read as a pull request from the future.
  it("shows a time ahead of now as no minutes at all", () => {
    expect(shortAge(now + 5 * minute, now)).toBe("0m");
  });
});

describe("refreshedStamp", () => {
  it("reads just now under a minute", () => {
    expect(refreshedStamp(now - 30_000, now)).toBe("Refreshed just now");
  });

  it("counts minutes, then hours, then days", () => {
    expect(refreshedStamp(now - 2 * minute, now)).toBe("Refreshed 2 min ago");
    expect(refreshedStamp(now - 3 * hour, now)).toBe("Refreshed 3 h ago");
    expect(refreshedStamp(now - 2 * day, now)).toBe("Refreshed 2 d ago");
  });

  it("reads just now for a time ahead of now", () => {
    expect(refreshedStamp(now + 5 * minute, now)).toBe("Refreshed just now");
  });
});
