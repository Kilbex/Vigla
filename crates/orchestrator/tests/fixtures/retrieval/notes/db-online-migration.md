# Run zero-downtime schema migrations in three phases

1. Deploy code that writes BOTH old and new columns, reads from
   the old one.
2. Backfill: a one-off task copies old → new for legacy rows.
3. Deploy code that reads from the new column and stops writing
   the old. Drop the old column in a later release once all
   replicas are on phase-3 code. Never combine phases.
