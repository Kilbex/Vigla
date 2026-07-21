# Memory kernel is single-writer per project

The Tier-2F design says one process at a time may write to a
project's `.vigla/memory/memory.sqlite`. The kernel takes a
file lock on open; a second process exits with
`MemoryError::Locked`. This is intentional — concurrent writers
would race the witness count and produce non-deterministic
promotion outcomes.
