// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import {
  getShowAllEvents,
  setShowAllEvents,
} from "../settings/preferences";

const KEY = "vigla.prefs.show_all_events.v1";

beforeEach(() => {
  window.localStorage.removeItem(KEY);
});

afterEach(() => {
  window.localStorage.removeItem(KEY);
});

describe("show-all-events preference", () => {
  it("defaults to false on a fresh install", () => {
    expect(getShowAllEvents()).toBe(false);
  });

  it("round-trips a true value", () => {
    setShowAllEvents(true);
    expect(getShowAllEvents()).toBe(true);
  });

  it("round-trips a false value", () => {
    setShowAllEvents(true);
    setShowAllEvents(false);
    expect(getShowAllEvents()).toBe(false);
  });

  it("treats a corrupted entry as false", () => {
    window.localStorage.setItem(KEY, "not-a-bool");
    expect(getShowAllEvents()).toBe(false);
  });
});
