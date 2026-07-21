//! Antigravity CLI adapter for line-oriented output.
//!
//! Every non-empty stdout line becomes an info `Log`,
//! every stderr line becomes an error `Log`. On finalize, synthesise
//! a terminal `state_change` plus a `completion` or `failure` based
//! on the child's exit status. This deliberately uses the shared raw-log
//! contract because Antigravity does not expose a stable structured event
//! stream that Vigla can safely depend on.

#![deny(missing_debug_implementations)]

#[derive(Debug)]
pub struct AntigravityAdapter {
    inner: adapter_core::RawLogAdapter,
}

impl AntigravityAdapter {
    pub fn new(worker_id: impl Into<String>, task_id: Option<String>) -> Self {
        Self {
            inner: adapter_core::RawLogAdapter::new(
                worker_id,
                task_id,
                "antigravity worker finished",
            ),
        }
    }
}

impl adapter_core::Adapter for AntigravityAdapter {
    fn ingest_line(
        &mut self,
        line: &str,
        stream: event_schema::LogStream,
    ) -> Vec<event_schema::Event> {
        self.inner.ingest_line(line, stream)
    }

    fn finalize(&mut self, exit: adapter_core::AdapterExit) -> Vec<event_schema::Event> {
        self.inner.finalize(exit)
    }

    fn take_quota_signal(&mut self) -> Option<adapter_core::QuotaSignal> {
        self.inner.take_quota_signal()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use adapter_core::Adapter;
    use event_schema::LogStream;

    #[test]
    fn quota_line_surfaces_signal_through_wrapper() {
        let mut a = AntigravityAdapter::new("w1", None);
        let _ = a.ingest_line("HTTP 429: rate limit exceeded", LogStream::Stderr);
        assert!(
            a.take_quota_signal().is_some(),
            "wrapper must delegate quota detection to its RawLogAdapter inner"
        );
        assert!(a.take_quota_signal().is_none());
    }
}
