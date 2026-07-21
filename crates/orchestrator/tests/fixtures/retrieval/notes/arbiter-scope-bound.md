# Scope bound trips before audit on out-of-scope writes

The pre-flight ACL gate in `acl/sentinel.rs` rejects a worker
submission whose touched paths fall outside `MissionSpec.scope_paths`
BEFORE any `AuditCompleted` event fires. This means a scope
violation emits exactly one `ArbiterDecided { bound: Scope }`
event and zero audit events — the right shape for the L1 row-2
substitute. Audit is not free; do not run it on dead-on-arrival
submissions.
