# Pipkin built-in engine reference

Use this reference when the user asks about Pipkin. It is compiled into the native
engine and is available without an external runtime or network access.

- Pipkin is a desktop coding assistant. The built-in Rust engine manages conversations,
  provider requests, tools, subagents, history and recovery. It executes tools in the
  conversation's project directory. Tool access is not a security sandbox.
- Desktop Connections supports Claude subscription and ChatGPT/Codex sign-in. Other
  built-in OAuth flows are accessible through the engine service. Saved subscription
  credentials remain distinct from API keys; failed refresh does not silently switch billing.
- Experimental local Claude CLI conversations use an installed, unmodified CLI and its own
  normal login, separately from native subscription sign-in. The built Linux desktop accepts
  stable CLI versions 2.1.299 or newer for chat and restricted Write, subject to capability, trust
  and login checks. Bindings record the actual version; existing identities are not rewritten.
  Live qualification evidence is limited to 2.1.299, not every accepted newer release. Normal owned
  desktop engines enable CLI hosting by default, without granting trust or readiness. Use
  `--disable-local-claude-cli` or `--disable-local-claude-cli-write` to opt out. Demo stays disabled;
  attached hosts own their policy, and standalone server/library defaults remain off. Choose
  Settings → Connections → Claude Code · local CLI. Setup is only in Connections, not a sidebar
  action or Ctrl+N chooser. Connections configures independent providers without selecting one.
  Choose Claude Code CLI Haiku/Sonnet/Opus in the normal model picker alongside native providers.
  Cross-provider or CLI-alias changes create a separate conversation; no history/draft is forwarded.
  Failed/cancelled changes block only their source until completed or explicitly kept; other chats
  remain usable. New follows the current conversation's model, or its explicit pending choice.
  The composer's model picker shows the actual model, with no separate Backend label or silent
  GPT/native fallback.
  Filesystem-only discovery finds the installation
  and captures the selected project; saved model/path preferences are not grants. Review canonical
  paths, then explicitly confirm Trust these paths and connect. Provider configuration stops
  after Check; connecting a conversation coordinates Check and one metadata-only Create/Return,
  not a prompt. Uncertain outcomes never automatically replay.
  Reconnect loads saved hints but requires fresh confirmation. Only Write is supported
  in the coding slice, not Read/Edit/Bash/MCP or native tools/goals/queues/retries. Review complete
  arguments before Allow once; Ctrl+Shift+C copies the current review without deciding. Unknown
  prompts/decisions must not be replayed. Billing, managed policy and full crash containment remain
  unverified. See docs/local-claude-cli.md for isolated testing and recovery.
- Amazon Bedrock supports AWS IAM Identity Center profiles and the AWS CLI SSO token cache.
  Run `aws sso login --profile <profile>` to establish the session, then select that profile
  through `AWS_PROFILE` or the Bedrock profile option. Session-based SSO profiles can refresh
  cached tokens; expired legacy SSO profiles require another CLI login. MFA role profiles
  requiring an interactive code callback need externally obtained temporary credentials.
- Bedrock also supports the separate `login_session` profiles created by `aws login`.
  It reads the AWS login cache, refreshes temporary credentials with signed proofs, and
  saves rotated tokens. `AWS_LOGIN_CACHE_DIRECTORY` overrides the default cache directory.
- Vertex AI supports service-account, authorized-user and impersonated-service-account ADC
  files. Its ADC mode can also use Google metadata credentials when no default file exists;
  the `gcp-vertex-credentials` key marker selects ADC without a local file. Configure the
  Google project and location as usual. `GOOGLE_CLOUD_QUOTA_PROJECT` overrides ADC billing.
- Vertex ADC also supports external-account file/URL subject tokens, workforce billing,
  optional service-account impersonation, and external-account authorized-user refresh.
  Executable ADC helpers require `GOOGLE_EXTERNAL_ACCOUNT_ALLOW_EXECUTABLES=1`; their
  configured command runs directly with Google's noninteractive environment. OIDC/SAML
  responses and helper-managed output caches are supported, with 5–120 second timeouts.
  AWS federation ADC uses AWS environment credentials or the metadata endpoints configured
  in that ADC file, including IMDSv2. This Google path does not read AWS CLI profiles.
  Certificate ADC supports explicit or default `certificate_config.json`, leaf/trust chains,
  and mutual TLS for credential exchanges while model requests use the resulting bearer token.
- The engine configuration directory holds `auth.json`, `models.json` and `settings.json`.
  Standalone engine-server flags `--engine-config-dir` and `--engine-data-dir` select engine
  configuration and history. Desktop `--data-dir` selects an isolated desktop/owned-engine
  profile; those server-only path flags are not desktop options. Do not expose stored credentials.
- Project instructions come from AGENTS.md or CLAUDE.md and their supported overrides.
  Skills load from the engine configuration's `skills` directory, the project's `.pi/skills`,
  and configured skill paths. Prompt templates load from the configuration's `prompts`
  and the project's `.pi/prompts`. These legacy directory names remain supported data paths.
  Live root/child groups share an instruction/skill snapshot; a fresh lifetime reloads it.
- Steering adds input at the next eligible boundary. Follow-ups wait for the current work.
  Stop retires the interrupted instruction from later model context; it does not undo file
  changes or other effects already performed. Check status after an uncertain submission
  instead of blindly sending it again. Restart recovers durable work; interrupted tools may
  have partially changed files and should be inspected before being repeated.
- Close in the conversation header moves a settled conversation into the sidebar's Closed section.
  Reopen restores the same conversation, history, draft and provider identity; nothing is deleted,
  cancelled or resubmitted. Closed membership persists locally, separately for each backend.
  Active/uncertain work, queued input, pending approvals, active goals and unsaved drafts must be
  resolved first. Current engine state must be fully loaded. Reported new activity restores a closed
  conversation's visibility without selecting it or replaying work. Selecting a closed history-search
  result explicitly reopens it.
- The Plan banner appears only while tasks are open and can be dismissed without cancelling work.
  A new independent task batch after all current tasks finish archives the previous plan; history
  remains readable. Explicit references to prior task IDs continue that plan instead. Unfinished
  work is never silently discarded. The plan inspector remains accessible after banner dismissal.
- Compaction summarizes earlier context while preserving browsable history. Model and
  thinking selections are conversation-specific. Usage includes provider-reported tokens;
  estimates and missing provider fields are not exact billing records.
- Legacy conversation and credential imports are explicit. Original data and backups are
  preserved; historical extension selections do not activate extensions. Legacy extension
  compatibility is not required for Pipkin.
- The Changes inspector reports Git state. A path mention references a path; it does not
  attach file contents. Read the relevant file before relying on it.
- For local diagnostics use `pipkin --engine native --diagnose --probe`; it does not submit
  a model request. Investigate reported failures before suggesting profile deletion.
