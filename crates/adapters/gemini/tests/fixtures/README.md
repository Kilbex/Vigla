# Gemini adapter fixtures

`happy_path.jsonl` — captured 2026-05-10 from a Gemini CLI release
reporting `gemini --version` 0.41.2, running this command from
`/tmp/vigla-gemini-fixture/`:

```bash
gemini --skip-trust \
  -p "Read README.md in this directory and summarize it in one sentence." \
  --output-format stream-json --approval-mode yolo
```

The directory contains a single one-paragraph `README.md`. The model
reads it via `read_file`, emits two streaming `delta` assistant
messages, then a final `result` with token-count stats.

`--skip-trust` is required for headless use in untrusted directories
(see https://geminicli.com/docs/cli/trusted-folders/). The supervisor's
`spawn_gemini` passes the same flag for the same reason.

## Observed line shape (key fields)

| `type`        | Notable fields                                     |
|---------------|----------------------------------------------------|
| `init`        | `session_id`, `model`                              |
| `message`     | `role` (`user` / `assistant`), `content`, `delta?` |
| `tool_use`    | `tool_name`, `tool_id`, `parameters`               |
| `tool_result` | `tool_id`, `status` (`success` / `error`), `output`|
| `result`      | `status`, `stats.{total,input,output}_tokens`      |

The version above is historical fixture metadata, not a supported-version
constraint. If a future Gemini CLI release changes the schema, re-capture
against the same prompt and update the adapter to match.
