# We use merge commits, not rebases

Vigla integrations land via `git merge --no-ff` so every
mission has a single visible parent edge on `supervisor/main`.
This makes `git log --first-parent supervisor/main` an exact
mission history and lets `RevertButton` revert the merge with one
SHA. Rebase would lose the boundary and force a multi-commit
revert.
