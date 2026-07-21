# Use spawn_blocking only for sync IO over 1ms

`tokio::task::spawn_blocking` has ~30µs of scheduling overhead so
wrapping a short sync call (a string parse, a HashMap lookup) makes
the program slower, not faster. Use it for file IO, SQLite calls
that don't have an async driver, and CPU-bound work over ~1ms. The
async-compatibility lint flags new spawn_blocking sites.
