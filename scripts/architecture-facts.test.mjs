import assert from "node:assert/strict";
import { readFile, readdir } from "node:fs/promises";
import test from "node:test";

const architecture = await readFile(
  new URL("../ARCHITECTURE.md", import.meta.url),
  "utf8",
);
const migrationFiles = await readdir(
  new URL("../crates/orchestrator/migrations", import.meta.url),
);

test("the migration range quoted in ARCHITECTURE.md reaches the newest migration", () => {
  const numbered = migrationFiles
    .map((name) => /^(\d{4})_.+\.sql$/.exec(name))
    .filter((match) => match !== null)
    .map((match) => match[1]);
  assert.ok(numbered.length > 0, "no numbered migrations found");

  const lowest = numbered.reduce((a, b) => (a < b ? a : b));
  const highest = numbered.reduce((a, b) => (a > b ? a : b));

  // The en dash is the one ARCHITECTURE.md actually uses; a hyphen here would
  // make this test pass against prose no reader ever sees.
  const quoted = /Migrations\s+(\d{4})–(\d{4})/.exec(architecture);
  assert.ok(
    quoted,
    "ARCHITECTURE.md no longer states a `Migrations NNNN–NNNN` range",
  );
  assert.equal(
    quoted[1],
    lowest,
    "quoted range starts below or above the earliest migration file",
  );
  assert.equal(
    quoted[2],
    highest,
    `ARCHITECTURE.md quotes migrations up to ${quoted[2]}, but crates/orchestrator/migrations/ now reaches ${highest}`,
  );
});

test("the migration directory has no gaps the quoted range would paper over", () => {
  const numbered = migrationFiles
    .map((name) => /^(\d{4})_.+\.sql$/.exec(name))
    .filter((match) => match !== null)
    .map((match) => Number.parseInt(match[1], 10))
    .sort((a, b) => a - b);

  const missing = [];
  for (let n = numbered[0]; n <= numbered[numbered.length - 1]; n += 1) {
    if (!numbered.includes(n)) missing.push(String(n).padStart(4, "0"));
  }
  assert.deepEqual(missing, [], "migration numbering is not contiguous");
});

test("ARCHITECTURE.md cites code by symbol, not by line number", () => {
  // Line offsets rot on the next commit and silently mislead a reader who
  // follows them. File + symbol survives a refactor or fails loudly.
  const lineRefs = architecture.match(/`[^`\n]*\.rs:\d+(?:-\d+)?`/g) ?? [];
  assert.deepEqual(lineRefs, []);
});
