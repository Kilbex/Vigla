# CLAUDE.md, AGENTS.md, GEMINI.md are RENDER TARGETS

The vendor CLI native memory files (`CLAUDE.md` for Claude Code,
`AGENTS.md` for Codex, `GEMINI.md` for Gemini) are written by the
kernel's render pass on every promotion. They are NEVER read back
as source of truth — the kernel's SQLite + notes/ tree owns truth.
Hand-edits to the vendor files get blown away on the next render.
