//! Tokeniser shared by every retrieval backend.
//!
//! Contract:
//!
//! - Splits on ASCII whitespace and ASCII punctuation.
//! - Lowercases each token (ASCII fold; Unicode characters pass
//!   through unchanged so non-English text isn't mangled).
//! - No stemming, no stopword removal, no Unicode normalisation.
//! - Empty inputs and pure-whitespace inputs return an empty `Vec`.
//!
//! Determinism > recall at this layer. BM25 scoring is reproducible
//! across runs because tokenisation is byte-deterministic; the V0
//! evaluation harness asserts this with a same-query-twice check.

/// Tokenise `s` into a `Vec<String>` of lowercase tokens, splitting
/// on ASCII whitespace and ASCII punctuation.
///
/// Apostrophes inside a word are treated as a split too — `"don't"`
/// becomes `["don", "t"]`. This is the simplest defensible rule and
/// matches how BM25 implementations in `tantivy` / `whoosh` handle
/// the default analyzer.
pub fn tokenize(s: &str) -> Vec<String> {
    if s.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut buf = String::new();
    for ch in s.chars() {
        let is_sep = ch.is_ascii_whitespace() || (ch.is_ascii_punctuation());
        if is_sep {
            if !buf.is_empty() {
                out.push(std::mem::take(&mut buf));
            }
            continue;
        }
        for low in ch.to_lowercase() {
            buf.push(low);
        }
    }
    if !buf.is_empty() {
        out.push(buf);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_yields_empty_vec() {
        assert_eq!(tokenize(""), Vec::<String>::new());
        assert_eq!(tokenize("   \t\n"), Vec::<String>::new());
    }

    #[test]
    fn lowercases_ascii() {
        assert_eq!(tokenize("Hello WORLD"), vec!["hello", "world"]);
    }

    #[test]
    fn splits_on_ascii_punctuation() {
        assert_eq!(
            tokenize("auth/authentication, JWT(token)"),
            vec!["auth", "authentication", "jwt", "token"]
        );
    }

    #[test]
    fn apostrophes_split_words() {
        assert_eq!(tokenize("don't can't"), vec!["don", "t", "can", "t"]);
    }

    #[test]
    fn preserves_non_ascii_characters() {
        // Non-ASCII punctuation (em dash) and letters pass through:
        // letters retained (lowercased), em dash is *not* ASCII
        // punctuation so it stays inside the token. This is the
        // conservative behaviour — we don't want to silently mangle
        // accented Latin or CJK text.
        let toks = tokenize("café — naïve");
        assert!(toks.iter().any(|t| t == "café"));
        assert!(toks.iter().any(|t| t == "naïve"));
    }

    #[test]
    fn deterministic_on_repeat() {
        let input = "BM25 + alias: auth ↔ authentication";
        assert_eq!(tokenize(input), tokenize(input));
    }
}
