---
name: Systematic debugging
description: Use when you hit a bug, test failure, or unexpected behavior — find the root cause before changing code.
scope: repo
enabled: true
priority: 90
---
When something is broken:

1. Reproduce it with the smallest possible input, ideally a failing test.
2. Read the actual error and the surrounding code before forming a theory.
3. Form ONE hypothesis, find the cheapest observation that would confirm or kill it, and check.
4. Fix the root cause, not the symptom. If you cannot explain *why* it broke, you are not done.
5. Re-run the reproduction to confirm the fix, then run the wider suite for regressions.
