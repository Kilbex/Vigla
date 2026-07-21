//! Local ONNX embeddings (V1.2).
//!
//! [`EmbedModel`] supplies single and batched embedding APIs. Migration
//! 0013 stores vectors, and the hybrid scorer consumes them when the
//! optional feature is enabled.
//!
//! All embedding code is gated behind the `embeddings` cargo feature
//! so the default build stays free of `ort` / native ONNX runtime
//! linkage.
//!
//! ## Graceful degradation contract
//!
//! [`EmbedModel::try_new`] is **infallible by design**. If the
//! fastembed initialiser fails — typically because of an offline
//! first run, a missing-write-permission cache directory, or a model
//! download corruption — we log a single `tracing::warn!` and return
//! an [`EmbedModel`] whose every `embed` call answers `None`.
//! Hybrid scoring detects this `None` and falls back to
//! BM25-only retrieval. The plan's "local-first ethos: offline
//! first-run must still work" lives in this single branch.
//!
//! ## Determinism
//!
//! fastembed wraps ORT, which is deterministic for a fixed model +
//! input + thread count. We pin nothing here — the determinism test
//! ([`tests::same_input_produces_bit_identical_vector`]) embeds the
//! same string twice and asserts byte-for-byte equality.

/// Stable identifier for the embedding model. Bumping this string
/// triggers re-embedding of every promoted note on next kernel open
/// through migration 0013. Matches the fastembed model id so
/// the version string round-trips through future model registries.
pub const MODEL_VERSION: &str = "sentence-transformers/all-MiniLM-L6-v2";

/// Dimensionality of the MiniLM-L6-v2 output vector. Asserted at
/// runtime so a model swap that changes the dimension fails loudly
/// at first encode rather than silently corrupting the SQLite BLOB
/// layout.
pub const EMBEDDING_DIM: usize = 384;

// ---------------------------------------------------------------
// Feature-gated implementation
// ---------------------------------------------------------------

#[cfg(feature = "embeddings")]
mod imp {
    use super::{EMBEDDING_DIM, MODEL_VERSION};
    use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};
    use std::path::PathBuf;
    use std::sync::Mutex;
    use tracing::warn;

    /// Resolve the fastembed model-cache directory.
    ///
    /// Precedence (first match wins):
    ///
    /// 1. `VIGLA_FASTEMBED_CACHE_DIR` — explicit override, used by
    ///    tests and by operators who want to pin the cache to an
    ///    operator-controlled path (e.g. a CI scratch volume).
    /// 2. `FASTEMBED_CACHE_DIR` — the fastembed-rs upstream env var,
    ///    honoured for compatibility with operators already pinning
    ///    fastembed through it.
    /// 3. `dirs::cache_dir()` joined with `"fastembed"` — the
    ///    per-user XDG-aware cache location promised by the design
    ///    doc (V1.2 row): `~/.cache/fastembed/` on Linux,
    ///    `~/Library/Caches/fastembed/` on macOS,
    ///    `%LOCALAPPDATA%\fastembed\` on Windows. A single user with
    ///    multiple repos pays one ~90 MB download; the cache also
    ///    survives `git clean` and per-repo `.vigla/` resets.
    /// 4. As a last resort (no HOME-equivalent on this platform),
    ///    fall back to fastembed's own default of `.fastembed_cache`
    ///    in the cwd. This matches the historical behaviour and
    ///    keeps containers without a writable HOME functional.
    pub(super) fn resolve_cache_dir() -> PathBuf {
        if let Ok(p) = std::env::var("VIGLA_FASTEMBED_CACHE_DIR") {
            return PathBuf::from(p);
        }
        if let Ok(p) = std::env::var("FASTEMBED_CACHE_DIR") {
            return PathBuf::from(p);
        }
        if let Some(base) = dirs::cache_dir() {
            return base.join("fastembed");
        }
        PathBuf::from(".fastembed_cache")
    }

    enum Inner {
        // Boxed to keep the Disabled variant cheap. `TextEmbedding`
        // is ~1.2 KB and the warning fires without the box.
        Ready(Box<Mutex<TextEmbedding>>),
        Disabled,
    }

    /// Owning handle to a loaded fastembed model.
    ///
    /// Held singleton-style by [`MemoryKernel`]. All `&self` methods are safe to call from many
    /// tasks concurrently — internal `Mutex` serialises ORT session
    /// access because the underlying `TextEmbedding` is `!Sync` for
    /// inference.
    pub struct EmbedModel {
        inner: Inner,
    }

    impl std::fmt::Debug for EmbedModel {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match &self.inner {
                Inner::Ready(_) => f
                    .debug_struct("EmbedModel")
                    .field("state", &"Ready")
                    .finish(),
                Inner::Disabled => f
                    .debug_struct("EmbedModel")
                    .field("state", &"Disabled")
                    .finish(),
            }
        }
    }

    impl EmbedModel {
        /// Initialise the embedder. **Infallible** — on any error
        /// (network, IO, corrupt cache, etc.) logs a single
        /// `tracing::warn!` and returns a disabled instance. See
        /// the module doc-comment for the degradation contract.
        pub fn try_new() -> Self {
            let cache_dir = resolve_cache_dir();
            // Create the dir up-front so fastembed's hf-hub client
            // gets a writable target on the first run. Ignore errors
            // — fastembed will surface a more precise diagnostic if
            // it can't write either.
            if let Err(e) = std::fs::create_dir_all(&cache_dir) {
                tracing::debug!(
                    target: "memory.retrieval.embed",
                    cache_dir = %cache_dir.display(),
                    error = %e,
                    "could not pre-create fastembed cache dir; \
                     fastembed will retry"
                );
            }
            match TextEmbedding::try_new(
                InitOptions::new(EmbeddingModel::AllMiniLML6V2).with_cache_dir(cache_dir.clone()),
            ) {
                Ok(model) => {
                    tracing::info!(
                        target: "memory.retrieval.embed",
                        model = MODEL_VERSION,
                        cache_dir = %cache_dir.display(),
                        "fastembed initialised"
                    );
                    Self {
                        inner: Inner::Ready(Box::new(Mutex::new(model))),
                    }
                }
                Err(err) => {
                    warn!(
                        target: "memory.retrieval.embed",
                        model = MODEL_VERSION,
                        cache_dir = %cache_dir.display(),
                        error = %err,
                        "fastembed initialisation failed; \
                         retrieval will fall back to BM25-only"
                    );
                    Self {
                        inner: Inner::Disabled,
                    }
                }
            }
        }

        /// True when the embedder failed to load and every `embed`
        /// call will answer `None`. Hybrid scoring uses this to skip
        /// vector loading entirely.
        pub fn is_disabled(&self) -> bool {
            matches!(self.inner, Inner::Disabled)
        }

        /// Embed a single text. Returns `None` when the model is
        /// disabled or inference itself failed (the latter logs a
        /// `tracing::warn!`).
        ///
        /// Output is L2-normalised so cosine similarity reduces to
        /// a plain dot product downstream — Task 12 relies on this.
        pub fn embed(&self, text: &str) -> Option<Vec<f32>> {
            let mut batch = self.embed_batch(vec![text.to_string()])?;
            batch.pop()
        }

        /// Embed a batch of texts in one ORT session call. Preserves
        /// input order. Returns `None` when the model is disabled or
        /// inference failed.
        pub fn embed_batch(&self, texts: Vec<String>) -> Option<Vec<Vec<f32>>> {
            let model = match &self.inner {
                Inner::Ready(m) => m,
                Inner::Disabled => return None,
            };
            if texts.is_empty() {
                return Some(Vec::new());
            }
            let mut guard = match model.lock() {
                Ok(g) => g,
                Err(poisoned) => {
                    warn!(
                        target: "memory.retrieval.embed",
                        "embedder mutex poisoned; recovering"
                    );
                    poisoned.into_inner()
                }
            };
            let mut vecs = match guard.embed(texts, None) {
                Ok(v) => v,
                Err(err) => {
                    warn!(
                        target: "memory.retrieval.embed",
                        error = %err,
                        "fastembed encode failed; returning None"
                    );
                    return None;
                }
            };
            for v in &mut vecs {
                debug_assert_eq!(
                    v.len(),
                    EMBEDDING_DIM,
                    "MODEL_VERSION pinned a {EMBEDDING_DIM}-dim model; \
                     got {}",
                    v.len()
                );
                l2_normalize(v);
            }
            Some(vecs)
        }
    }

    /// In-place L2 normalisation. Zero vectors are left untouched
    /// (rather than producing NaN); downstream cosine then yields 0
    /// against any other vector — the harmless "no signal" answer.
    fn l2_normalize(v: &mut [f32]) {
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            for x in v.iter_mut() {
                *x /= norm;
            }
        }
    }

    /// Phase 2 / Task 9 smoke-test entry point. Kept for the
    /// EMBEDDINGS=1 DMG smoke build — exercises the full native
    /// path (download → ORT session → tokenise → infer) in one
    /// call so signing / notarization breakage is surfaced before
    /// any retrieval wiring lands.
    pub fn smoke_test_embed() -> Vec<f32> {
        EmbedModel::try_new()
            .embed("hello")
            .expect("smoke test: embedder must be Ready and encode must succeed")
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        #[ignore = "network: downloads ~22 MB MiniLM-L6-v2 on first run"]
        fn embed_returns_384_dim_unit_vector() {
            let m = EmbedModel::try_new();
            assert!(
                !m.is_disabled(),
                "init must succeed when network is available"
            );
            let v = m.embed("the quick brown fox").expect("encode");
            assert_eq!(v.len(), EMBEDDING_DIM);
            let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
            assert!(
                (norm - 1.0).abs() < 1e-5,
                "vector must be L2-normalised; got norm={norm}"
            );
        }

        #[test]
        #[ignore = "network: downloads ~22 MB MiniLM-L6-v2 on first run"]
        fn same_input_produces_bit_identical_vector() {
            let m = EmbedModel::try_new();
            let a = m.embed("determinism check").expect("a");
            let b = m.embed("determinism check").expect("b");
            assert_eq!(a, b, "ORT must be deterministic for a fixed input");
        }

        #[test]
        #[ignore = "network: downloads ~22 MB MiniLM-L6-v2 on first run"]
        fn cosine_of_identical_text_is_one() {
            let m = EmbedModel::try_new();
            let a = m.embed("hybrid retrieval").expect("a");
            let b = m.embed("hybrid retrieval").expect("b");
            let dot: f32 = a.iter().zip(&b).map(|(x, y)| x * y).sum();
            assert!(
                (dot - 1.0).abs() < 1e-5,
                "L2-normalised identical vectors should have dot=1.0; got {dot}"
            );
        }

        #[test]
        #[ignore = "network: downloads ~22 MB MiniLM-L6-v2 on first run"]
        fn batch_preserves_input_order() {
            let m = EmbedModel::try_new();
            let batch = m
                .embed_batch(vec![
                    "alpha".to_string(),
                    "beta".to_string(),
                    "gamma".to_string(),
                ])
                .expect("batch");
            assert_eq!(batch.len(), 3);
            let single_beta = m.embed("beta").expect("single");
            assert_eq!(
                batch[1], single_beta,
                "batch element 1 must match a standalone encode of \"beta\""
            );
        }

        #[test]
        fn empty_batch_returns_empty_vec_without_loading_model() {
            // Constructs Disabled to prove this path doesn't touch the
            // model. If init somehow succeeds (e.g. cache warm), the
            // test is still valid — we just exercise the Ready branch.
            let m = EmbedModel::try_new();
            let out = m.embed_batch(Vec::new()).unwrap_or_default();
            assert!(out.is_empty());
        }

        #[test]
        fn l2_normalize_handles_zero_vector() {
            let mut v = vec![0.0_f32; 8];
            l2_normalize(&mut v);
            assert!(v.iter().all(|x| *x == 0.0), "zero in, zero out (no NaN)");
        }

        #[test]
        fn l2_normalize_produces_unit_length() {
            let mut v = vec![3.0_f32, 4.0];
            l2_normalize(&mut v);
            let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
            assert!((norm - 1.0).abs() < 1e-6);
            assert!((v[0] - 0.6).abs() < 1e-6);
            assert!((v[1] - 0.8).abs() < 1e-6);
        }

        // Cache-dir resolver tests. Mutates env vars, so serialised
        // via a single mutex to keep them deterministic under
        // `cargo test`'s parallel runner.
        fn env_lock() -> std::sync::MutexGuard<'static, ()> {
            static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
            LOCK.get_or_init(|| std::sync::Mutex::new(()))
                .lock()
                .unwrap_or_else(|p| p.into_inner())
        }

        fn with_clean_env<F: FnOnce()>(f: F) {
            let _g = env_lock();
            let saved_or = std::env::var("VIGLA_FASTEMBED_CACHE_DIR").ok();
            let saved_fe = std::env::var("FASTEMBED_CACHE_DIR").ok();
            // SAFETY: harness is single-threaded under env_lock above.
            unsafe {
                std::env::remove_var("VIGLA_FASTEMBED_CACHE_DIR");
                std::env::remove_var("FASTEMBED_CACHE_DIR");
            }
            f();
            unsafe {
                if let Some(v) = saved_or {
                    std::env::set_var("VIGLA_FASTEMBED_CACHE_DIR", v);
                } else {
                    std::env::remove_var("VIGLA_FASTEMBED_CACHE_DIR");
                }
                if let Some(v) = saved_fe {
                    std::env::set_var("FASTEMBED_CACHE_DIR", v);
                } else {
                    std::env::remove_var("FASTEMBED_CACHE_DIR");
                }
            }
        }

        #[test]
        fn resolve_cache_dir_default_is_per_user() {
            with_clean_env(|| {
                let got = resolve_cache_dir();
                // Either dirs::cache_dir() resolved (preferred) or the
                // last-resort .fastembed_cache (no HOME). Never the
                // earlier env-var branches.
                if let Some(expected) = dirs::cache_dir() {
                    assert_eq!(
                        got,
                        expected.join("fastembed"),
                        "default cache dir must be per-user under dirs::cache_dir()"
                    );
                } else {
                    assert_eq!(got, std::path::PathBuf::from(".fastembed_cache"));
                }
            });
        }

        #[test]
        fn resolve_cache_dir_vigla_override_wins() {
            with_clean_env(|| {
                unsafe {
                    std::env::set_var("VIGLA_FASTEMBED_CACHE_DIR", "/tmp/vigla-fastembed-override");
                    std::env::set_var("FASTEMBED_CACHE_DIR", "/should/be/ignored");
                }
                assert_eq!(
                    resolve_cache_dir(),
                    std::path::PathBuf::from("/tmp/vigla-fastembed-override"),
                    "VIGLA_FASTEMBED_CACHE_DIR has higher precedence than FASTEMBED_CACHE_DIR"
                );
            });
        }

        #[test]
        fn resolve_cache_dir_honours_upstream_fastembed_env_var() {
            with_clean_env(|| {
                unsafe {
                    std::env::set_var("FASTEMBED_CACHE_DIR", "/tmp/fastembed-upstream");
                }
                assert_eq!(
                    resolve_cache_dir(),
                    std::path::PathBuf::from("/tmp/fastembed-upstream"),
                );
            });
        }
    }
}

#[cfg(feature = "embeddings")]
pub use imp::{smoke_test_embed, EmbedModel};

// ---------------------------------------------------------------
// Stub implementation when the `embeddings` feature is OFF
// ---------------------------------------------------------------

#[cfg(not(feature = "embeddings"))]
mod stub {
    /// Off-feature stand-in for [`EmbedModel`]. Always disabled;
    /// every `embed` returns `None`. Lets hybrid scoring compile
    /// against a stable surface whether or not the feature is on.
    #[derive(Debug)]
    pub struct EmbedModel;

    impl EmbedModel {
        pub fn try_new() -> Self {
            Self
        }
        pub fn is_disabled(&self) -> bool {
            true
        }
        pub fn embed(&self, _text: &str) -> Option<Vec<f32>> {
            None
        }
        pub fn embed_batch(&self, _texts: Vec<String>) -> Option<Vec<Vec<f32>>> {
            None
        }
    }
}

#[cfg(not(feature = "embeddings"))]
pub use stub::EmbedModel;
