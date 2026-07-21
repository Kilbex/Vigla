# Adapter Conformance Harness

Golden-transcript contract tests for Vigla vendor adapters. Each case
pairs a recorded CLI transcript with a committed golden snapshot of the
adapter's *entire* output — the canonical event stream plus the
side-channel drains (`session_id`, `quota_signal`, `memory_intents`,
`context_requests`). Any drift in adapter behavior fails the test, which
runs as part of `cargo test --workspace` in CI.

## Layout (per adapter crate)

    tests/
      conformance.rs                      # test driver (one #[test] per case)
      conformance/
        <case>.transcript.json            # recorded input (lines + finalize)
        <case>.golden.json                # committed expected snapshot

## Transcript format

    {
      "lines": [ { "stream": "stdout" | "stderr", "text": "<raw CLI line>" } ],
      "finalize": null | "Clean" | "Killed" | "Failed" | "Failed:<code>"
    }

`finalize: null` means the CLI emitted its own terminal line. The other
values invoke `Adapter::finalize` with the matching `AdapterExit`.

## Updating goldens

After an **intentional** adapter change, regenerate and review the diff:

    UPDATE_GOLDEN=1 cargo test -p <crate> --test conformance
    git diff -- '*/conformance/*.golden.json'   # review every change

A missing golden is a hard error (never a silent pass), so new cases must
be generated explicitly. The wall-clock `ts` field is redacted to `<ts>`;
everything else is asserted verbatim.

## Adding a vendor

1. Add `adapter-conformance` under `[dev-dependencies]`.
2. Copy `tests/conformance.rs`, swap the adapter constructor.
3. Author `<case>.transcript.json` files using that vendor's real wire
   shapes; generate goldens; review; commit.
