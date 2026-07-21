# Quota signal renders a paused mission card

When a Claude or Codex adapter drains a `QuotaSignal { reset_at }`,
the orchestrator scheduler enters `MissionState::QuotaPaused` and
emits one `QuotaPauseStarted` event. The MissionInbox surfaces a
card with a live countdown to `reset_at`. Quota pauses are
nondeterministic in CI and gated under L3 dogfood, not L1.
