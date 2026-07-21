-- Vigla Memory Kernel — Tier-1 hardening.
--
-- Witness signals must be unique per causal source. Without this,
-- re-running an accept/scrub barrier (or any other re-invocation of
-- `witnesses::record`) silently shifts confidence by inserting
-- duplicate rows. The kernel also guards idempotence at the operation
-- level (barrier-event existence check before reflection), but this
-- constraint is the durable defense.
--
-- `source_event_id` is the event whose effect this witness records:
--   * UserAuthored               — the `note_authored` event
--   * UserAccepted / UserScrubbed — the `barrier` event for the mission
--   * WorkerProposed             — the `ratified` event for the proposal
--   * DerivedFromUntrustedFile   — the `ratified` event for the proposal
--   * TestPassedAfterUse / TestFailedAfterUse — the test_result event id
--   * ReviewApproved             — the PR-merge event id (P6)

CREATE UNIQUE INDEX idx_mem_wit_unique
    ON memory_witnesses(note_id, kind, source_event_id);
