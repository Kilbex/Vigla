---
name: Verify before done
description: Use before claiming work is complete — prove it with commands, do not assume.
scope: repo
enabled: true
priority: 70
---
Before reporting success:

1. Run the project's build, tests, and linter; paste the ACTUAL output, not the expected output.
2. Re-read your diff line by line: does every change trace to the task? Remove orphans you created.
3. Check the edges: empty input, error paths, and the contract callers depend on.
4. State plainly what you verified and what you did not. Never claim more than the evidence supports.
