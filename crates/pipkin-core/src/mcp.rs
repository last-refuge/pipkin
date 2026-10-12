//! MCP servers as the desktop shows and manages them (the Connections page's "MCP servers"
//! section). The engine owns the configuration and the processes; this state mirrors its
//! view and carries the person's in-progress edits. Secret values never reach it: the
//! engine redacts literals and never sends tokens.

use crate::onboarding::SignInAnswer;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum McpScope {
    User,
    Project,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum McpKind {
    Stdio,
    Http,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum McpServerState {
    Stopped,
    Connecting,
    Ready,
    AuthRequired,
    Failed,
}

impl McpServerState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Stopped => "Stopped",
            Self::Connecting => "Connecting",
            Self::Ready => "Connected",
            Self::AuthRequired => "Sign-in needed",
            Self::Failed => "Failed",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum McpAuthStatus {
    #[default]
    Idle,
    Connecting,
    Waiting,
    Prompt,
    Done,
    Failed,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct McpPrompt {
    pub id: String,
    /// `text`, `secret`, `select` or `manual_code`.
    pub kind: String,
    pub message: String,
    pub placeholder: Option<String>,
}

/// Sign-in state of one server. The URL is volatile presentation state, never persisted.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct McpAuth {
    pub supported: bool,
    pub signed_in: bool,
    pub required: bool,
    pub status: McpAuthStatus,
    pub message: String,
    pub url: Option<String>,
    pub prompt: Option<McpPrompt>,
}

impl std::fmt::Debug for McpAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpAuth")
            .field("supported", &self.supported)
            .field("signed_in", &self.signed_in)
            .field("required", &self.required)
            .field("status", &self.status)
            .field("url", &self.url.as_ref().map(|_| "[url]"))
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpTool {
    pub name: String,
    pub description: String,
    pub enabled: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpServer {
    pub name: String,
    pub scope: McpScope,
    pub kind: McpKind,
    pub enabled: bool,
    pub state: McpServerState,
    pub error: Option<String>,
    /// Cached tool count, when the catalog has been read.
    pub tools: Option<usize>,
    pub catalog: Vec<McpTool>,
    pub disabled_tools: Vec<String>,
    /// Resources and prompt templates offered, once listed.
    pub resources: Option<usize>,
    pub prompts: Option<usize>,
    /// What the server declared: `tools`, `resources`, `prompts`, ...
    pub capabilities: Vec<String>,
    /// The command line, or the URL.
    pub command: String,
    /// `NAME=$REF` / `NAME=secret:key` / `NAME=…` (literals are never shown).
    pub env: Vec<String>,
    pub keep_alive: bool,
    pub idle_seconds: u64,
    pub in_flight: usize,
    pub auth: McpAuth,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct McpSnapshot {
    pub servers: Vec<McpServer>,
    pub warnings: Vec<String>,
    /// The open conversation's project can hold its own entries.
    pub project_available: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct McpTestResult {
    pub name: String,
    pub ok: bool,
    pub message: String,
    pub tools: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpImportEntry {
    pub name: String,
    pub kind: String,
    /// The command line or URL of the converted entry.
    pub summary: String,
    pub warnings: Vec<String>,
    /// A server of this name is already configured (applying replaces it).
    pub exists: bool,
    pub valid: bool,
    pub selected: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct McpImportPreview {
    /// The pasted text the preview came from; applying sends it back with the chosen names.
    pub text: String,
    pub entries: Vec<McpImportEntry>,
    pub warnings: Vec<String>,
}

impl McpImportPreview {
    pub fn chosen(&self) -> Vec<String> {
        self.entries
            .iter()
            .filter(|e| e.selected && e.valid)
            .map(|e| e.name.clone())
            .collect()
    }
}

/// What the add form sends: a stdio command line or an HTTP URL plus `NAME=value` pairs
/// (environment for stdio, headers for HTTP). Values may be literals or references
/// (`$NAME`, `secret:key`); the engine stores them and never shows literals again.
#[derive(Clone, PartialEq, Eq)]
pub struct McpServerInput {
    pub scope: McpScope,
    pub name: String,
    pub kind: McpKind,
    pub target: String,
    pub values: Vec<(String, String)>,
}

impl std::fmt::Debug for McpServerInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpServerInput")
            .field("scope", &self.scope)
            .field("name", &self.name)
            .field("kind", &self.kind)
            .field("target", &self.target)
            .field(
                "values",
                &self.values.iter().map(|(k, _)| k).collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl McpServerInput {
    /// `NAME=value` lines into pairs; blank lines and lines without `=` are skipped.
    pub fn parse_values(text: &str) -> Vec<(String, String)> {
        text.lines()
            .filter_map(|line| {
                let line = line.trim();
                let (k, v) = line.split_once('=')?;
                let k = k.trim();
                (!k.is_empty()).then(|| (k.to_owned(), v.trim().to_owned()))
            })
            .collect()
    }

    /// Why the input cannot be sent, if it cannot.
    pub fn problem(&self) -> Option<String> {
        let name = self.name.trim();
        if name.is_empty() {
            return Some("Give the server a name.".into());
        }
        if name.len() > 64
            || !name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
        {
            return Some("Names use letters, digits, - _ and . only.".into());
        }
        let target = self.target.trim();
        if target.is_empty() {
            return Some(match self.kind {
                McpKind::Stdio => "Give the command that starts the server.".into(),
                McpKind::Http => "Give the server's URL.".into(),
            });
        }
        if self.kind == McpKind::Http
            && !(target.starts_with("http://") || target.starts_with("https://"))
        {
            return Some("The URL must start with http:// or https://.".into());
        }
        None
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct McpState {
    /// The backend manages MCP servers (the built-in engine does; the demo does not).
    pub available: bool,
    pub servers: Vec<McpServer>,
    pub warnings: Vec<String>,
    pub project_available: bool,
    pub import: Option<McpImportPreview>,
    pub test: Option<McpTestResult>,
    /// What the backend is doing for the page right now ("Testing docs…").
    pub busy: Option<String>,
    /// The last refused management action.
    pub notice: Option<String>,
    /// The server whose tools are listed.
    pub expanded: Option<String>,
}

impl McpState {
    pub fn server(&self, name: &str) -> Option<&McpServer> {
        self.servers.iter().find(|s| s.name == name)
    }

    pub fn apply(&mut self, snapshot: McpSnapshot) {
        self.available = true;
        self.servers = snapshot.servers;
        self.warnings = snapshot.warnings;
        self.project_available = snapshot.project_available;
        self.busy = None;
        if let Some(expanded) = &self.expanded
            && !self.servers.iter().any(|s| &s.name == expanded)
        {
            self.expanded = None;
        }
    }
}

/// An answer to a server's sign-in prompt (a pasted code or redirect URL).
pub type McpSignInAnswer = SignInAnswer;
