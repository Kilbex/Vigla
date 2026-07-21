---
name: Test-first
description: Use before implementing a feature or bugfix — write the failing test before the code.
scope: repo
enabled: true
priority: 80
---
Before writing implementation code:

1. Write a test that asserts the behavior you want and currently fails.
2. Run it and confirm it fails for the expected reason (not a typo/compile error).
3. Write the minimum code to make it pass.
4. Run the test and confirm it passes.
5. Refactor only with the test green. Keep changes surgical; do not touch unrelated code.
