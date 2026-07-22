## Summary

<!-- What changed and why? -->

## Testing

<!-- Commands run, screenshots, or "not run" with reason. -->

## Risk and Rollback

<!-- What could regress, and how can a maintainer undo or contain it? -->

## Scope Check

- [ ] The change is focused and does not add unrelated refactors.
- [ ] Business logic stays outside the Tauri host unless this PR is specifically about host integration.
- [ ] Adapter changes are covered by fixture or parser tests when practical.
- [ ] User-visible behavior changes are described above.
- [ ] New dependencies are justified; `cargo deny --all-features check` and
  `pnpm audit --audit-level low` remain clean.
- [ ] Public contracts or persisted data changes include compatibility notes.
- [ ] Documentation and screenshots match the behavior being shipped.

## Notes

<!-- Optional follow-ups, risks, or review hints. -->
