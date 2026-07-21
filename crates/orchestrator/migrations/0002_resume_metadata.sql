-- Step 25 — resume metadata.
--
-- A worker can now span multiple supervised child processes over its
-- lifetime: the initial spawn captures the vendor CLI's session id,
-- and follow-up invocations re-spawn the CLI with `--resume <id>` plus
-- a new prompt. Both fields are nullable because legacy workers and
-- vendors without a resume primitive will never populate them.

ALTER TABLE workers ADD COLUMN session_id  TEXT;
ALTER TABLE workers ADD COLUMN last_prompt TEXT;
