//! Alias dictionary + expansion (V1.1).
//!
//! Tokens are post-tokenisation, lowercase. Aliases are symmetric:
//! `auth ↔ authentication` means *either* expands to *both*. Stored
//! as a flat `HashMap<token, Vec<token>>` keyed by the surface token;
//! [`AliasDict::seed_default`] populates both directions so the
//! lookup is a single `O(1)` per token. The plan ships a seed
//! dictionary from `docs/lexicon.md` highlights plus common
//! shorthands. The launch format is intentionally bundled-only: changing
//! aliases is a reviewed code change, which keeps ranking deterministic.
//!
//! Expansion is order-preserving and deduplicated:
//! `["db", "tuning"]` against `{ "db" → ["database"] }` produces
//! `["db", "database", "tuning"]`, never `["db", "database", "tuning",
//! "db"]`.

use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Default)]
pub struct AliasDict {
    map: HashMap<String, Vec<String>>,
}

impl AliasDict {
    pub fn new() -> Self {
        Self::default()
    }

    /// Bundled seed dictionary. Pairs are intentionally conservative
    /// — over-eager aliasing tanks precision (every `auth` query
    /// would also pull `authoritative` notes). Stick to abbreviations
    /// and direct synonyms.
    pub fn seed_default() -> Self {
        let pairs: &[(&str, &[&str])] = &[
            ("auth", &["authentication", "authn"]),
            ("authn", &["authentication", "auth"]),
            ("authentication", &["auth", "authn"]),
            ("authz", &["authorization", "authorisation"]),
            ("db", &["database", "sqlite", "postgres"]),
            ("database", &["db"]),
            ("ui", &["frontend", "webview"]),
            ("frontend", &["ui"]),
            ("cli", &["command-line", "commandline"]),
            ("pr", &["pull-request", "pullrequest"]),
            ("e2e", &["end-to-end", "endtoend"]),
            ("ci", &["continuous-integration"]),
            ("dag", &["graph", "task-graph"]),
            ("revert", &["rollback", "undo"]),
            ("rollback", &["revert", "undo"]),
            ("undo", &["revert", "rollback"]),
            ("merge", &["integration", "integrate"]),
            ("integrate", &["merge"]),
            ("integration", &["merge"]),
            ("supervisor", &["arbiter"]),
            ("arbiter", &["supervisor"]),
            ("worker", &["employee"]),
            ("employee", &["worker"]),
            ("escalate", &["escalation"]),
            ("quota", &["rate-limit", "ratelimit"]),
            ("paused", &["pause", "paused-mission"]),
            ("crash", &["segfault", "segv", "panic"]),
            ("timeout", &["hang", "stuck"]),
            ("flaky", &["flake", "intermittent", "race"]),
            ("race", &["race-condition", "flaky"]),
            ("h1", &["heading", "headline", "title"]),
            ("title", &["headline", "h1"]),
            ("headline", &["title", "h1"]),
            ("frontmatter", &["header", "preamble"]),
            ("playwright", &["browser-test", "e2e"]),
            ("vitest", &["unit-test"]),
            ("jest-dom", &["testing-library", "dom-matchers"]),
            ("jsdom", &["dom"]),
            ("tauri", &["webview", "desktop-app"]),
            ("invoke", &["ipc"]),
            ("listen", &["event-bus", "subscribe"]),
            ("clippy", &["lint", "linter"]),
            ("lint", &["clippy"]),
            ("warnings", &["unused", "dead-code", "warning"]),
            ("pedantic", &["clippy-pedantic"]),
            ("rebase", &["rewrite"]),
            ("snapshot", &["pre-merge", "tag"]),
            ("snap", &["snapshot", "tag"]),
            ("tag", &["snapshot", "pre-merge"]),
            ("mock", &["stub", "fake"]),
            ("mocks", &["stubs", "fakes"]),
            ("witness", &["promotion-witness", "witnesses"]),
            ("witnesses", &["witness"]),
            ("promotion", &["promote", "promoted"]),
            ("promote", &["promotion", "promoted"]),
            ("kernel", &["memory-kernel"]),
            ("acl", &["sentinel", "scope-gate"]),
            ("sentinel", &["acl"]),
            ("schema", &["migration"]),
            ("migration", &["schema"]),
            ("backfill", &["repair", "rebuild-index"]),
            ("pool", &["connection-pool", "pool-size"]),
            ("pem", &["public-key", "key"]),
            ("jwt", &["json-web-token"]),
            ("rs256", &["rsa-256", "rsa256"]),
            ("downtime", &["zero-downtime", "online"]),
            ("online", &["zero-downtime"]),
            ("countdown", &["timer", "rate-limit-pause"]),
            ("incremental", &["cache", "cached-build"]),
            ("vendor", &["adapter", "cli"]),
            ("adapter", &["vendor", "cli"]),
            ("vendored", &["bundled"]),
            ("hidden", &["visibility", "background"]),
            ("visibility", &["hidden", "visibilitystate"]),
            ("scope", &["scope-paths", "boundary"]),
            ("boundary", &["scope", "envelope"]),
            ("envelope", &["authority", "boundary"]),
            ("authority", &["envelope"]),
            ("inbox", &["mission-inbox", "notifications"]),
            ("dialog", &["modal", "confirmation"]),
            ("modal", &["dialog"]),
            ("button", &["control"]),
        ];
        let mut map = HashMap::new();
        for (k, vs) in pairs {
            map.insert(
                (*k).to_string(),
                vs.iter().map(|s| (*s).to_string()).collect(),
            );
        }
        Self { map }
    }

    pub fn insert(&mut self, canonical: impl Into<String>, aliases: Vec<String>) {
        self.map.insert(canonical.into(), aliases);
    }

    pub fn aliases_of(&self, term: &str) -> Option<&[String]> {
        self.map.get(term).map(|v| v.as_slice())
    }
}

/// Expand `tokens` against `dict`, preserving order and deduplicating.
/// Empty `dict` is a no-op (returns the input tokens cloned).
pub fn expand_aliases(tokens: &[String], dict: &AliasDict) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(tokens.len() * 2);
    let mut seen: HashSet<String> = HashSet::new();
    for t in tokens {
        if seen.insert(t.clone()) {
            out.push(t.clone());
        }
        if let Some(aliases) = dict.aliases_of(t) {
            for a in aliases {
                if seen.insert(a.clone()) {
                    out.push(a.clone());
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_dict_is_a_no_op() {
        let dict = AliasDict::new();
        let toks = vec!["auth".into(), "jwt".into()];
        assert_eq!(expand_aliases(&toks, &dict), toks);
    }

    #[test]
    fn expansion_preserves_order_and_dedupes() {
        let dict = AliasDict::seed_default();
        let toks: Vec<String> = vec!["db".into(), "tuning".into(), "db".into()];
        let out = expand_aliases(&toks, &dict);
        // First element stays first, db's aliases (database/sqlite/
        // postgres) follow, then tuning, and the second "db" is a
        // duplicate so it does not repeat.
        assert_eq!(out[0], "db");
        assert!(out.contains(&"database".to_string()));
        assert!(out.contains(&"tuning".to_string()));
        // No duplicate "db" anywhere after position 0.
        let db_count = out.iter().filter(|t| t.as_str() == "db").count();
        assert_eq!(db_count, 1);
    }

    #[test]
    fn alias_lookup_is_symmetric_for_seeded_pair() {
        let dict = AliasDict::seed_default();
        // auth ↔ authentication is bidirectionally seeded.
        assert!(dict
            .aliases_of("auth")
            .unwrap()
            .iter()
            .any(|t| t == "authentication"));
        assert!(dict
            .aliases_of("authentication")
            .unwrap()
            .iter()
            .any(|t| t == "auth"));
    }
}
