//! Engine diagnostics as the desktop shows them (plan §12, OBS-1): a summary of the
//! engine's own measurements for the open conversation (provider and tool latency, retries,
//! queue waits, cancellations) and a bounded page of its newest records, fetched on request.
//! These are engine-measured model and tool figures; the desktop's input-to-frame latency is
//! a separate measurement and is never mixed in here.

/// Figures over the engine's retained records of one session. Milliseconds throughout.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DiagnosticsSummary {
    pub records: u64,
    pub runs: u64,
    pub provider_requests: u64,
    pub provider_errors: u64,
    pub retries: u64,
    pub cancellations: u64,
    pub tool_calls: u64,
    pub tool_errors: u64,
    pub provider_p50_ms: Option<u64>,
    pub provider_p95_ms: Option<u64>,
    pub first_token_p50_ms: Option<u64>,
    pub tool_p50_ms: Option<u64>,
    pub tool_p95_ms: Option<u64>,
    pub queue_wait_max_ms: Option<u64>,
    pub stop_latency_max_ms: Option<u64>,
}

/// One record, already put into words by the adapter (bounded; no bodies).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiagnosticRecord {
    pub seq: u64,
    pub at_ms: i64,
    pub kind: String,
    pub run: Option<u64>,
    pub call_id: Option<String>,
    pub line: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DiagnosticsState {
    /// The engine answered at least once: the section is shown.
    pub available: bool,
    pub summary: Option<DiagnosticsSummary>,
    /// Newest first, bounded by the adapter.
    pub records: Vec<DiagnosticRecord>,
    pub loading: bool,
    pub exporting: bool,
    pub error: Option<String>,
    /// Where the last export was written.
    pub exported: Option<String>,
}

impl DiagnosticsState {
    pub(crate) fn reset(&mut self) {
        *self = DiagnosticsState::default();
    }
}

/// `1.2 s`, `350 ms`.
pub fn format_ms(ms: u64) -> String {
    if ms >= 10_000 {
        format!("{} s", ms / 1000)
    } else if ms >= 1000 {
        let tenths = (ms + 50) / 100;
        format!("{}.{} s", tenths / 10, tenths % 10)
    } else {
        format!("{ms} ms")
    }
}
