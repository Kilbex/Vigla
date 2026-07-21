# Kernel-rendered frontmatter has no title field

`render_note_file` in `memory/store.rs` emits a YAML-ish header
with `id`, `kind`, `scope`, `created_at`, `schema_version` — but
NOT `title`. The title is derived at INSERT time from the body's
first H1 and stored in the `memory_notes.title` column. Do not
add a `title:` line to the rendered file; `strip_frontmatter` will
discard it on read and the file content will drift from the DB.
