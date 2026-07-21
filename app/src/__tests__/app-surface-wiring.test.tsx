// S10 — verifies the App.tsx right-rail surface switch. Mocks
// the Tauri bindings so the test focuses on the new wiring; the
// other overlays (drawer / settings / mission overlay) lazy-load
// behind Suspense and don't render in the default state.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render } from "@testing-library/react";

vi.mock("../bindings", async (orig) => {
  const realModule = (await orig()) as Record<string, unknown>;
  // Wrap commands in a Proxy that returns a no-op async function for
  // any unknown property — the App pulls in many command surfaces
  // and listing them all here would drift.
  const baseCommands = (realModule.commands ?? {}) as Record<string, unknown>;
  const commandsProxy = new Proxy(baseCommands, {
    get(target, prop) {
      if (prop === "listRecentMissions") {
        return vi.fn().mockResolvedValue({ status: "ok", data: [] });
      }
      if (prop === "checkCliAuth") {
        return vi.fn().mockResolvedValue([]);
      }
      if (prop in target) return target[prop as string];
      return vi.fn().mockResolvedValue({ status: "ok", data: null });
    },
  });
  return {
    ...realModule,
    events: {
      workerEvent: { listen: vi.fn().mockResolvedValue(() => {}) },
      missionEventDto: { listen: vi.fn().mockResolvedValue(() => {}) },
    },
    commands: commandsProxy,
  };
});

import App from "../App";
import { useSurfaceStore } from "../inbox/router";
import { setShowAllEvents } from "../settings/preferences";

beforeEach(() => {
  useSurfaceStore.setState({ surface: "inbox", detail: null });
  setShowAllEvents(false);
});

afterEach(() => {
  setShowAllEvents(false);
});

describe("App surface wiring", () => {
  it("renders InboxOverview by default (surface=inbox)", () => {
    const { container } = render(<App />);
    expect(container.querySelector(".inbox-overview")).not.toBeNull();
    expect(container.querySelector(".comms-feed")).toBeNull();
  });

  it("keeps the deploy form visible on the default empty app surface", () => {
    const { container } = render(<App />);
    expect(container.querySelector(".operations-room-launch")).not.toBeNull();
    expect(container.querySelector(".deploy-panel")).not.toBeNull();
  });

  it("renders CommsFeed when surface=ops_room and showAllEvents on", () => {
    setShowAllEvents(true);
    useSurfaceStore.setState({ surface: "ops_room", detail: null });
    const { container } = render(<App />);
    expect(container.querySelector(".comms-feed")).not.toBeNull();
  });

  it("renders MissionHistory when surface=history", () => {
    useSurfaceStore.setState({ surface: "history", detail: null });
    const { container } = render(<App />);
    expect(container.querySelector(".mission-history")).not.toBeNull();
  });

  it("renders MissionInbox when surface=mission_detail", () => {
    useSurfaceStore.setState({
      surface: "mission_detail",
      detail: { missionId: "m1" },
    });
    const { container } = render(<App />);
    expect(container.querySelector(".mission-inbox")).not.toBeNull();
  });
});
