# Shareable mission templates

Status: product specification for later roadmap work. No template file is
accepted as a stable public contract until the schema and security review land.

## Outcome

A user can save, review, share, and instantiate a reusable mission definition
for recurring work such as dependency updates, test backfills, or release
chores. A template describes intent and bounds; it never silently grants
authority or executes when opened.

## Smallest useful schema

The first version should cover:

- schema version and human-readable name;
- objective with explicit placeholder fields;
- worker roster by capability/vendor preference, not credentials;
- Scope, Reversibility, Risk, and Quality bounds;
- allowed path patterns and required test commands;
- plan-review mode and maximum retry/continuation policy;
- optional explanatory metadata and source URL.

Do not include API keys, session identifiers, absolute home-directory paths,
embedded shell secrets, model transcripts, or an automatic-run flag.

## Import safety

- Parse into a typed inert preview before touching the mission store.
- Reject unknown major schema versions; preserve unknown minor fields when
  round-tripping if compatibility permits.
- Display every authority-bearing value and local path substitution.
- Require explicit confirmation before saving or launching an imported
  template.
- Treat remote templates as untrusted data and cap field sizes and collection
  counts.
- Record provenance without making a remote URL a live dependency.

## Candidate starter set

- dependency update with lockfile and test bounds;
- regression-test backfill with production-source changes forbidden;
- release-note and changelog preparation;
- adapter fixture addition with credential-free tests;
- documentation consistency sweep.

Starter templates must be honest examples, not privileged built-ins: users can
inspect, copy, edit, and delete them.

## Acceptance gate

1. a versioned JSON Schema and compatibility policy are committed;
2. malformed, oversized, unknown-version, and secret-shaped fixtures are
   rejected safely;
3. import preview and explicit launch confirmation are browser- and unit-tested;
4. template execution uses the existing mission validation path rather than a
   parallel shortcut;
5. exported templates are deterministic and contain no machine-local data;
6. at least three starter templates round-trip and complete in the mock
   harness;
7. documentation names which fields are suggestions versus enforced bounds.

Open questions—vendor fallback semantics, placeholder syntax, signing, and a
community catalog—must be decided from concrete fixtures, not speculative
abstractions.
