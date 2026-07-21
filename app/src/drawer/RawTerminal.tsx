import { FitAddon } from "@xterm/addon-fit";
import { SearchAddon } from "@xterm/addon-search";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { Terminal } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";
import { useEffect, useRef } from "react";
import type { Event } from "../bindings";
import { eventsAfterSeq } from "./terminal-cursor";

interface RawTerminalProps {
  events: Event[];
  /// The worker whose events are being rendered. Used to detect when
  /// the parent (Drawer) switches between workers without unmounting
  /// — at that point we reset the seq cursor and clear the terminal,
  /// otherwise lastSeqRef carries over from worker A and worker B's
  /// fresh seqs (which start near 0) get filtered out by
  /// `eventsAfterSeq`, leaving the terminal blank.
  workerId: string;
}

const STATE_COLOR: Record<string, string> = {
  idle: "\x1b[37m", // gray
  planning: "\x1b[33m", // yellow
  executing: "\x1b[36m", // cyan
  blocked: "\x1b[31m", // red-ish
  reviewing: "\x1b[34m", // blue
  done: "\x1b[32m", // green
  failed: "\x1b[91m", // bright red
};
const RESET = "\x1b[0m";
const DIM = "\x1b[2m";

function renderLine(event: Event): string {
  const seqStr = `#${event.seq.toString().padStart(3, " ")}`;
  const dim = `${DIM}${seqStr}${RESET}`;
  switch (event.type) {
    case "state_change": {
      const c = STATE_COLOR[event.payload.state] ?? "";
      const note = event.payload.note ? ` ${DIM}${event.payload.note}${RESET}` : "";
      const from = event.payload.from ? `${DIM} (from ${event.payload.from})${RESET}` : "";
      return `${dim} ${c}state${RESET} → ${c}${event.payload.state}${RESET}${from}${note}`;
    }
    case "log": {
      const levelColor =
        event.payload.level === "error"
          ? "\x1b[91m"
          : event.payload.level === "warn"
            ? "\x1b[33m"
            : event.payload.level === "info"
              ? "\x1b[37m"
              : "\x1b[2m";
      return `${dim} ${levelColor}${event.payload.level}${RESET} ${event.payload.line}`;
    }
    case "progress":
      return `${dim} ${DIM}progress${RESET} ${event.payload.percent.toFixed(1)}%${event.payload.note ? ` — ${event.payload.note}` : ""}`;
    case "file_activity":
      return `${dim} \x1b[36mfile${RESET} ${event.payload.op} ${event.payload.path}`;
    case "test_result": {
      const fail = event.payload.failed > 0 ? `\x1b[91m${event.payload.failed} fail${RESET}` : `${event.payload.failed} fail`;
      return `${dim} \x1b[34mtests${RESET} ${event.payload.suite} → ${event.payload.passed} pass / ${fail} / ${event.payload.skipped} skip`;
    }
    case "cost":
      return `${dim} ${DIM}cost${RESET} +$${event.payload.usd.toFixed(4)}`;
    case "dependency":
      return `${dim} \x1b[33mdep${RESET} waiting on ${event.payload.waiting_on.join(", ")}`;
    case "completion":
      return `${dim} \x1b[32mdone${RESET} ${event.payload.summary}`;
    case "failure":
      return `${dim} \x1b[91mfail${RESET} ${event.payload.error}`;
  }
}

/// Raw terminal tab. The mock workers write JSONL; for fidelity with
/// real CLIs (Step 10+) we render a colorized transcript here. Real
/// CLI adapters will feed the original stdout text directly.
export default function RawTerminal({ events, workerId }: RawTerminalProps) {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const termRef = useRef<Terminal | null>(null);
  const fitRef = useRef<FitAddon | null>(null);
  // Track the last seq we wrote to the terminal, not an array index.
  // The store's per-worker event log is bounded (MAX_EVENTS_PER_WORKER)
  // and rotates the oldest events; an index-based cursor would freeze
  // at length=cap and silently stop printing once the rotation begins.
  const lastSeqRef = useRef<number | null>(null);

  useEffect(() => {
    if (!containerRef.current) return;
    const rootStyles = window.getComputedStyle(document.documentElement);
    const themeToken = (name: string, fallback: string) =>
      rootStyles.getPropertyValue(name).trim() || fallback;
    const terminalBackground = themeToken("--terminal-bg", "#030712");
    const terminalForeground = themeToken("--terminal-fg", "#eef2f7");
    const term = new Terminal({
      convertEol: true,
      fontFamily:
        '"JetBrains Mono", "SF Mono", ui-monospace, Menlo, Monaco, monospace',
      fontSize: 12,
      lineHeight: 1.35,
      theme: {
        background: terminalBackground,
        foreground: terminalForeground,
        cursor: themeToken("--accent", "#38c5b4"),
        black: terminalBackground,
        red: "#ff5c5c",
        green: "#5ac2a8",
        yellow: "#f5c45a",
        blue: "#7ab8ff",
        magenta: "#a56cff",
        cyan: "#3fffd0",
        white: terminalForeground,
        brightBlack: "#3a4453",
        brightRed: "#ff8a50",
        brightGreen: "#5ac2a8",
        brightYellow: "#f5c45a",
        brightBlue: "#7ab8ff",
        brightMagenta: "#a56cff",
        brightCyan: "#3fffd0",
        brightWhite: terminalForeground,
      },
      scrollback: 5000,
      cursorBlink: false,
      disableStdin: true,
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.loadAddon(new SearchAddon());
    term.loadAddon(new WebLinksAddon());
    term.open(containerRef.current);
    try {
      fit.fit();
    } catch {
      // ignore — happens when container is briefly 0-size
    }
    termRef.current = term;
    fitRef.current = fit;

    const onResize = () => {
      try {
        fit.fit();
      } catch {
        /* ignore */
      }
    };
    window.addEventListener("resize", onResize);
    return () => {
      window.removeEventListener("resize", onResize);
      term.dispose();
      termRef.current = null;
      fitRef.current = null;
      lastSeqRef.current = null;
    };
  }, []);

  // Audit-r5 polish — reset the seq cursor + clear the terminal
  // whenever the parent switches workers without unmounting us.
  // Drawer keeps `RawTerminal` mounted across worker switches as long
  // as `tab === "terminal"`; without this effect the previous worker's
  // last seq would filter out the new worker's events.
  useEffect(() => {
    const term = termRef.current;
    if (!term) return;
    term.clear();
    lastSeqRef.current = null;
  }, [workerId]);

  // Append new events as they arrive.
  useEffect(() => {
    const term = termRef.current;
    if (!term) return;
    if (events.length === 0) return;
    const fresh = eventsAfterSeq(events, lastSeqRef.current);
    for (const e of fresh) {
      term.writeln(renderLine(e));
    }
    lastSeqRef.current = events[events.length - 1].seq;
  }, [events]);

  return <div ref={containerRef} className="drawer-terminal" />;
}
