# Gemini adapter fixtures

`happy_path.jsonl` is a hand-authored, deterministic synthetic fixture. It
models a session that reads `README.md`, emits two streaming `delta` assistant
messages, and finishes with token-count stats. Its identifiers, timestamps,
model names, durations, and usage counts are fixed fixture values; none came
from a user account or CLI session.

## Observed line shape (key fields)

| `type`        | Notable fields                                     |
|---------------|----------------------------------------------------|
| `init`        | `session_id`, `model`                              |
| `message`     | `role` (`user` / `assistant`), `content`, `delta?` |
| `tool_use`    | `tool_name`, `tool_id`, `parameters`               |
| `tool_result` | `tool_id`, `status` (`success` / `error`), `output`|
| `result`      | `status`, `stats.{total,input,output}_tokens`      |

If a future Gemini CLI release changes the schema, update this synthetic sample
and the adapter together without retaining session-specific metadata.
