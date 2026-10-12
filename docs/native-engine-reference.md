# Pipkin built-in engine reference

Use this reference when the user asks about Pipkin. It is compiled into the native
engine and is available without a Pi installation or network access.

- Pipkin is a desktop coding assistant. The built-in Rust engine manages conversations,
  provider requests, tools, subagents, history and recovery. It executes tools in the
  conversation's project directory. Tool access is not a security sandbox.
- Desktop Connections supports Claude subscription and ChatGPT/Codex sign-in. Other
  built-in OAuth flows are accessible through the engine service. Saved subscription
  credentials remain distinct from API keys; failed refresh does not silently switch billing.
- Amazon Bedrock supports AWS IAM Identity Center profiles and the AWS CLI SSO token cache.
  Run `aws sso login --profile <profile>` to establish the session, then select that profile
  through `AWS_PROFILE` or the Bedrock profile option. Session-based SSO profiles can refresh
  cached tokens; expired legacy SSO profiles require another CLI login. MFA role profiles
  requiring an interactive code callback need externally obtained temporary credentials.
- Bedrock also supports the separate `login_session` profiles created by `aws login`.
  It reads the AWS login cache, refreshes temporary credentials with signed proofs, and
  saves rotated tokens. `AWS_LOGIN_CACHE_DIRECTORY` overrides the default cache directory.
- The engine configuration directory holds `auth.json`, `models.json` and `settings.json`.
  The desktop's `--engine-config-dir` and `--engine-data-dir` select engine configuration
  and history; `--data-dir` selects desktop drafts/cache. Do not expose stored credentials.
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
- Compaction summarizes earlier context while preserving browsable history. Model and
  thinking selections are conversation-specific. Usage includes provider-reported tokens;
  estimates and missing provider fields are not exact billing records.
- Pi conversation and credential imports are explicit. Original data and backups are
  preserved; historical extension selections do not activate extensions. Pi extension
  compatibility is not required for Pipkin.
- The Changes inspector reports Git state. A path mention references a path; it does not
  attach file contents. Read the relevant file before relying on it.
- For local diagnostics use `pipkin --engine native --diagnose --probe`; it does not submit
  a model request. Investigate reported failures before suggesting profile deletion.
