# Retrieval Evaluation Fixtures

Hand-curated corpus for the memory retrieval evaluation harness
(`tests/memory_retrieval_evaluation.rs`, behind the
`retrieval-evaluation` cargo feature). Used to compute Recall@3 and
MRR for each V0 → V1.1 → V1.2 → V1.3 milestone.

## Layout

- `notes/<id>.md` — 20 notes. Each file is the raw note body that
  the harness feeds into `kernel.pin_note(...)`. First line is a
  Markdown H1; the harness asserts `extract_title` recovers it.
- `golden.json` — 30 query tuples. Schema:

  ```json
  {
    "query":           "free-form text passed as ContextRequest.detail",
    "expected_top_1":  "<note_id> | null",  // null = no-match expected
    "expected_top_3":  ["<id>", ...]         // up to 3 acceptable ids
  }
  ```

  When `expected_top_1` is `null`, the query is an explicit no-match
  case (the corpus is intentionally narrow on that topic). Predicted
  must be empty for credit on Recall@3 + MRR.

## Note kind distribution

20 notes spread across the 4 `StandardNoteKind` variants so the
retriever exercises each kind path:

- 5 `Hazard` — gotchas, footguns, known-bad recipes
- 5 `Fact`   — definitions, invariants, behavioural truths
- 5 `Procedure` — step-by-step recipes
- 5 `Decision` — recorded "we chose X over Y" with rationale

## Query mix

The 30 queries are intentionally lopsided toward what V0 (substring
scan) can answer, so the V0 baseline is a meaningful floor rather
than a near-zero noise number:

- 15 obvious — at least one rare keyword from the target note's
  H1 or body appears verbatim in the query
- 10 paraphrase — semantically equivalent to the target note but
  no shared content words (these will tank V0; V1.1 BM25 + alias
  dictionary will recover some; V1.2 embeddings should clear them)
- 5 no-match — terms the corpus genuinely doesn't cover

## Updating

When tuning V1.1 BM25 parameters or expanding the alias dictionary,
update `golden.json` only if a query was *factually* wrong (the
"expected" id is not actually the best answer in the corpus). Do not
edit goldens to make a regression "pass" — that's exactly the
regression-guard property the V0 → V1.1 test catches.
