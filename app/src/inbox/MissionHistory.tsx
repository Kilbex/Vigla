// S10 — MissionHistory. Cross-mission "last 20 audited missions"
// table. Reads from `list_recent_missions` Tauri command; row
// click opens the per-mission detail view via the surface router.

import { useEffect, useState } from "react";
import { commands } from "../bindings";
import { loadMissionTrustSnapshot } from "../missions/trustSnapshot";
import RiskRowOverallCell from "./RiskRowOverallCell";
import { useSurfaceStore } from "./router";
import type { MissionHistoryRow } from "./bindings-shim";

const LIMIT = 20;

function fmt(iso: string): string {
  const d = new Date(iso);
  if (isNaN(d.getTime())) return iso;
  const pad = (value: number) => value.toString().padStart(2, "0");
  return `${pad(d.getMonth() + 1)}/${pad(d.getDate())} ${pad(
    d.getHours(),
  )}:${pad(d.getMinutes())}`;
}

function fullFmt(iso: string): string {
  const d = new Date(iso);
  return isNaN(d.getTime()) ? iso : d.toLocaleString();
}

function statusLabel(status: MissionHistoryRow["status"]): string {
  return status.charAt(0).toUpperCase() + status.slice(1);
}

type LoadState =
  | { kind: "loading" }
  | { kind: "ok"; rows: MissionHistoryRow[] }
  | { kind: "error"; message: string };

export default function MissionHistory() {
  const openMission = useSurfaceStore((s) => s.openMission);
  const [state, setState] = useState<LoadState>({ kind: "loading" });

  useEffect(() => {
    let cancelled = false;
    commands
      .listRecentMissions(LIMIT)
      .then((result) => {
        if (cancelled) return;
        if (result.status === "ok") {
          setState({ kind: "ok", rows: result.data });
        } else {
          setState({ kind: "error", message: result.error });
        }
      })
      .catch((err: unknown) => {
        if (!cancelled) {
          setState({
            kind: "error",
            message: typeof err === "string" ? err : String(err),
          });
        }
      });
    return () => {
      cancelled = true;
    };
  }, []);

  return (
    <section className="mission-history" aria-label="Mission history">
      <header className="mission-history-header">
        <span className="mission-history-title">Recent missions</span>
      </header>
      {state.kind === "loading" ? (
        <div className="mission-history-empty">loading…</div>
      ) : null}
      {state.kind === "error" ? (
        <div className="mission-history-error" role="alert">
          {state.message}
        </div>
      ) : null}
      {state.kind === "ok" && state.rows.length === 0 ? (
        <div className="mission-history-empty">no missions yet</div>
      ) : null}
      {state.kind === "ok" && state.rows.length > 0 ? (
        <table className="mission-history-table">
          <thead>
            <tr>
              <th scope="col">Mission</th>
              <th scope="col">Audit</th>
              <th scope="col">Tier</th>
              <th scope="col">When</th>
              <th scope="col">Status</th>
            </tr>
          </thead>
          <tbody>
            {state.rows.map((row) => {
              const snapshot = loadMissionTrustSnapshot(row.mission_id);
              const label = snapshot?.title ?? row.mission_id;
              const status = row.reverted
                ? "Reverted"
                : statusLabel(row.status);
              const open = () => openMission(row.mission_id, row);
              return (
                <tr
                  key={`${row.mission_id}:${row.created_at}`}
                  className={[
                    "mission-history-row",
                    row.reverted ? "mission-history-row--reverted" : "",
                  ]
                    .filter(Boolean)
                    .join(" ")}
                  onClick={open}
                  onKeyDown={(e) => {
                    if (e.key === "Enter" || e.key === " ") {
                      e.preventDefault();
                      open();
                    }
                  }}
                  role="button"
                  tabIndex={0}
                  aria-label={`Open mission ${label}`}
                >
                  <td className="mission-history-mission" title={row.mission_id}>
                    <span className="mission-history-mission-title">{label}</span>
                    {label !== row.mission_id ? (
                      <span className="mission-history-mission-id">
                        {row.mission_id}
                      </span>
                    ) : null}
                  </td>
                  <RiskRowOverallCell overall={row.audit_overall} />
                  <td className="mission-history-tier">{row.tier}</td>
                  <td className="mission-history-when" title={fullFmt(row.created_at)}>
                    {fmt(row.created_at)}
                  </td>
                  <td className="mission-history-status">
                    <span
                      className={
                        row.reverted
                          ? "mission-history-reverted-pill"
                          : row.status === "merged"
                            ? "mission-history-merged-pill"
                            : "mission-history-active-pill"
                      }
                    >
                      {status}
                    </span>
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      ) : null}
    </section>
  );
}
