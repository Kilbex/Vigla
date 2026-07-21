# dag_fan_in_runs_in_dependency_order is a 1ms race

The `coordination::dag_fan_in_runs_in_dependency_order` test
asserts a strict event ordering using `now_ms()` timestamps. On
loaded macOS hosts two events occasionally land in the same
millisecond and the assertion flips. Re-running the file in
isolation always passes. Do not "fix" by relaxing the ordering
assertion; that loses the load-bearing dependency-order guarantee.
Track via the known-flake list; rerun on red.
