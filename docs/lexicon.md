# Vigla Lexicon

Single source-of-truth vocabulary. Every term used in the UI,
docs, and error messages is defined here. The README's lexicon
section is a thin pointer to this file.

| Term            | Definition (≤20 words)                                                                          | Where it surfaces                                          | Related concepts                                |
|-----------------|--------------------------------------------------------------------------------------------------|-------------------------------------------------------------|--------------------------------------------------|
| `envelope`      | The user's authority boundary on a mission — scope, risk, quality, reversibility.                | DeployPanel "Advanced" section; mission spec               | `bound`, `scope_paths`, `escalate`               |
| `envelope fit`  | The supervisor's four-bound (Scope / Reversibility / Risk / Quality) self-assessment of a plan.  | MissionPlanPreview overlay; Drawer's Plan tab              | `envelope`, `mind map`, `plan mode`              |
| `mind map`      | React Flow projection of a plan — root + tech-stack + wave nodes + task nodes + dependency edges. | MissionPlanPreview overlay; Drawer's Plan tab              | `envelope fit`, `plan mode`                      |
| `plan mode`     | Direct (default) — supervisor proceeds within the envelope. Review — user touch before workers.  | Settings panel; DeployPanel Advanced                       | `envelope fit`, `mind map`                       |
| `inbox`         | The mission inbox surface — the user's primary view of what needs attention vs what shipped.     | Right-rail by default; ⌘1                                  | `escalate`, `revert`, `card`                     |
| `escalate`      | A supervisor decision to surface a worker submission for user input rather than auto-deciding.   | Inbox card with `ActionRequired` severity                  | `bound`, `arbiter`                               |
| `revert`        | Creating a target-branch revert commit that undoes one merged mission while preserving later work. | MissionInbox revert button; MissionHistory reverted pill   | `rollback anchor`, `mission_revert_log`          |
| `roster`        | The team of AI workers visible in the operations room.                                           | Operations Room canvas                                     | `worker`, `squad`                                |
| `worker`        | A single AI CLI (Claude Code, Codex CLI, Antigravity, legacy Gemini, or mock).                   | Operations Room worker tiles; drawer                       | `vendor`, `adapter`                              |
| `mission`       | The unit of work — a goal assigned to one or more workers, with a target ref and an envelope.    | DeployPanel; MissionOverlay; inbox cards                   | `task`, `supervisor`, `arbiter`                  |
| `dispatch`      | Starting a worker on a mission task (one supervisor decision → one running worker).              | Operations Room squad bar; supervisor log                  | `mission`, `worker`                              |
| `brief`         | The mission's prompt + context, given to a worker at dispatch time.                              | Drawer "Brief" tab                                         | `handoff`, `mission`                             |
| `handoff`       | An explicit cross-worker note left by an upstream task for a downstream task in the DAG.         | Power-user feed; persisted in `memory_handoffs`            | `task graph`, `DAG`                              |
| `debrief`       | Reviewing the event feed for a completed mission.                                                | Drawer event feed; replay mode                             | `replay`, `event`                                |
| `accept`        | Approve the mission atomically — merge into trunk.                                               | MissionReviewOutcome; arbiter decision                     | `scrub`, `arbiter`                               |
| `scrub`         | Reject the mission atomically — discard, trunk clean, side effects logged.                       | MissionReviewOutcome; arbiter decision                     | `accept`, `side effect`                          |
| `recall`        | Stop a worker mid-mission.                                                                       | Drawer "Stop" button                                       | `dispatch`, `worker`                             |
| `arbiter`       | The supervisor's role in the supervisor-final-arbiter model — auto-deciding within the envelope. | All terminal decisions; ArbiterDecided event              | `supervisor`, `envelope`, `bound`                |
| `bound`         | One of the four authority bounds: Scope / Reversibility / Risk / Quality.                        | Inbox card "bound" label on Escalate decisions             | `envelope`, `escalate`                           |
| `residual risk` | The verdict-time risk band (Low / Medium / High) for a merged mission.                           | MissionInbox header badge; MissionHistory risk column      | `verdict`, `recommendation`                      |
| `verdict`       | The structured completion judgment — test pass, risk band, unresolved issues, recommendation.   | MissionInbox; `mission.completion_verdict_rendered` event  | `audit`, `recommendation`                        |
| `audit`         | The pre-decision quality check — composite score from test/scope/regression/lint scorers.       | AuditBreakdown component; `supervisor.audit_completed`     | `verdict`, `score`, `tier`                       |
| `tier`          | Audit depth: Smoke (≤30s) / Standard (≤2min) / Deep (≤5min).                                    | AuditBreakdown tier label                                  | `audit`, `score`                                 |

## Cross-references

- `envelope` (user input contract) ↔ `arbiter` (supervisor autonomy)
  ↔ `bound` (runtime check).
- `escalate` is the inverse of arbiter autonomy: a tripped bound
  surfaces as an Inbox card.
- `revert` is the user's hard-undo; durable final-merge anchors remain after
  intermediate snapshot compaction so History does not offer a dead control.
- `verdict` generalises the old TestResult — the audit score
  is a *component* of the verdict, not the whole signal.

## See also

- Visibility: `crates/orchestrator/src/escalation/`
- Audit: `crates/orchestrator/src/audit/`
- Completion judgment: `crates/orchestrator/src/judgment/README.md`
