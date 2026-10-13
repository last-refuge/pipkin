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

/// The engine's trace exporter as it reports itself (OBS-2); app-wide, not per conversation.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExportHealth {
    pub enabled: bool,
    /// `off`, `idle`, `sending`, `backoff` or `failed`.
    pub state: String,
    /// The collector's host only.
    pub endpoint: Option<String>,
    /// `metadata` or `content`.
    pub content: String,
    pub kind: String,
    pub service_name: Option<String>,
    /// The configured endpoint URL (no query string), for the settings page.
    pub endpoint_url: Option<String>,
    /// The stored credential's header name; its value never reaches the desktop.
    pub credential: Option<String>,
    /// Header values sit in the settings file; the page suggests moving them.
    pub headers_in_settings: bool,
    pub queued: u64,
    pub sent: u64,
    pub dropped: u64,
    pub failures: u32,
    pub last_error: Option<String>,
    pub last_success_ms: Option<i64>,
}

impl ExportHealth {
    /// One line for the session dialog.
    pub fn summary(&self) -> String {
        if !self.enabled {
            return "Off".into();
        }
        let mut parts = vec![format!(
            "{} to {}",
            match self.state.as_str() {
                "idle" => "Idle",
                "sending" => "Sending",
                "backoff" => "Retrying",
                "failed" => "Failing",
                other => other,
            },
            self.endpoint.as_deref().unwrap_or("?")
        )];
        parts.push(format!("{} sent", self.sent));
        if self.queued > 0 {
            parts.push(format!("{} waiting", self.queued));
        }
        if self.dropped > 0 {
            parts.push(format!("{} dropped", self.dropped));
        }
        if let Some(error) = &self.last_error {
            parts.push(error.clone());
        }
        parts.join(" · ")
    }
}

/// The Trace export settings page's own state (the health is app-wide too).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExportPage {
    /// What the backend is doing for the page ("Saving…").
    pub busy: Option<String>,
    /// The last refusal.
    pub notice: Option<String>,
    /// The last payload preview and the content level it was made at.
    pub preview: Option<(String, String)>,
}

/// What the person asked the exporter to be.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportConfig {
    pub enabled: bool,
    pub endpoint: String,
    /// `metadata` or `content`.
    pub content: String,
    pub service_name: String,
}
