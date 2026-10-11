# Pi audit, part 27: system prompt construction; settings, config, env, data layout

All paths are relative to `ROOT/packages/`. "CA" means `coding-agent/src`.

## 2. System prompt construction

### 2.1 Two builders, one core

- **Core:** `buildSystemPromptSections()` (CA/core/system-prompt.ts:121-180). It returns an ordered `Record<string,string>`.
  - `preamble` is untagged.
  - Every other key is wrapped as `<${name}>\n${content}\n</${name}>` (system-prompt.ts:175-178).
  - Section names must match `/^[a-z][a-z0-9_-]*$/` and must not be `preamble` (system-prompt.ts:52, 136-140).
- **Flat text:** `getSystemMessageText()` concatenates `content` and then each non-null section value. It drops empty parts and joins with `"\n\n"` (ai/src/utils/text.ts:15-21). `buildSystemPrompt()` is that applied to `buildSystemPromptState()` (system-prompt.ts:186-197).
- **Forced prompt:** `forceSystemPrompt` comes from an extension `before_agent_start` handler. It returns `{content: forced}` with no sections (system-prompt.ts:190).
- **Classic AgentSession:** `_rebuildSystemPrompt()` (CA/core/agent-session.ts:1652-1677) uses the loader's SYSTEM.md (`customPrompt`), APPEND_SYSTEM.md (`appendSystemPrompt`, multiple sources joined `"\n\n"`, :1663), context files, skills, and per-tool snippets/guidelines from the registry.
  - Hidden tools and tools without a snippet are not listed (:1655-1659, :1694-1697).
  - Changes are sent as a section diff: `diffSystemPromptSections()` (system-prompt.ts:204-216). A changed value is set, and a key that disappeared is set to `null`.
- **Durable server:** `createPiPrompt()` (CA/experimental/durable/prompt.ts:26-74). It is installed by `createCodingRegistry()` (CA/experimental/durable/harness-setup.ts:52-57).
  - Fixed key order: `["preamble","tools","rules","docs","project_context","skills","cwd"]` (prompt.ts:20). Each key becomes a durable `section(key, render, {tag:false})` because the built text already carries its tags (prompt.ts:69-73).
  - **No `addendum`** (APPEND_SYSTEM.md is ignored), **no SYSTEM.md** (no `customPrompt`), and no `promptGuidelines`.
  - cwd is `input.env?.cwd ?? input.agent.cwd ?? fallbackCwd` (prompt.ts:51).
  - Context files and skills load once per cwd and are cached in a Map (prompt.ts:27-39). The build is memoized per `PromptInput` in a WeakMap (prompt.ts:41-49).
  - Only the four tools in `CONTRIBUTIONS` (read/bash/edit/write) get snippets/guidelines (prompt.ts:12-17, 55-60).
  - Durable `CodingTools` order is `read, write, edit, bash` (durable/src/tools/index.ts:19-22). That order drives both the tools list and the rule order.
- **Durable harness tagging:** `renderSections()` (durable/src/harness/prompt.ts:25-48) wraps a section as `<key>\n…\n</key>` unless `tag === false`. It omits a section on `undefined`. If `render` throws, it keeps the previously shown text and reports the error.
  - `planSystemEntries()` (prompt.ts:66-93) writes `pi.system` entries in one of three forms:
    - a full baseline after a compaction head marker;
    - a minimal patch;
    - a remove-all + re-add-all pair when the patch would change key order (prompt.ts:122-142).
  - Tool add/remove rides on the last entry. Called from durable/src/harness/generation.ts:150-151.
- **Mid-conversation updates:** for APIs that accept them, an update renders as `Updated system prompt section "${name}":\n\n${value}` or `Removed system prompt section "${name}".` (ai/src/utils/text.ts:28-40).
- **Date / time: NOT injected anywhere.** There is no date in system-prompt.ts, durable prompt.ts, harness prompt.ts, or agent/src. The only dynamic values are cwd and paths.

### 2.2 Section order and exact text (default, no SYSTEM.md)

The order is the object insertion order (system-prompt.ts:142-173): `preamble`, `tools`, `rules`, `docs`, then `addendum` (classic only), `project_context`, `skills`, `cwd`, then custom `sections` (extension-supplied, appended last).

**preamble** (system-prompt.ts:146-147, verbatim):
```
You are an expert coding assistant operating inside pi, a coding agent harness. You help users by reading files, executing commands, editing code, and writing new files.
```
If a SYSTEM.md / `--system-prompt` value is present, `preamble = customPrompt` and **tools, rules and docs are omitted entirely** (system-prompt.ts:143-145).

**tools** (system-prompt.ts:148-151):
- Each line is `- ${name}: ${snippet}`. Only tools that are in `selectedTools` and have a snippet are listed, in `selectedTools` order. If none qualify, the text is `(none)`.
- The list is followed by `\n\nIn addition to the tools above, you may have access to other custom tools depending on the project.`
- The default `selectedTools` is `["read","bash","edit","write"]` (system-prompt.ts:58; also `DEFAULT_TOOL_NAMES`, CA/core/settings-manager.ts:215).

**rules** (`buildRules`, system-prompt.ts:81-118). Bullets are `- ${rule}`, trimmed, with exact-text dedupe, in this order:
1. If bash and/or powershell is selected and none of grep/find/ls is:
   - both: `Use bash or PowerShell for file operations like listing, searching, and finding files`
   - powershell only: `Use PowerShell for file operations like listing, searching, and finding files`
   - otherwise: `Use bash for file operations like ls, rg, find`
2. Each selected tool's guidelines, in `selectedTools` order.
3. `promptGuidelines` (from extensions).
4. `Be concise in your responses`
5. `Show file paths clearly when working with files`

**Tool contributions (verbatim):**

| tool | snippet | guidelines | ref |
|---|---|---|---|
| read | `Read file contents` | `Use read to examine files instead of cat or sed.` | CA/core/tools/read.ts:20-23 |
| bash | `Execute bash commands (ls, grep, find, etc.)` | `You can inspect PI_* environment variables for current model and session details.` | CA/core/tools/bash.ts:45-48 |
| edit | `Make precise file edits with exact text replacement, including multiple disjoint edits in one call` | 4 bullets, below | CA/core/tools/edit.ts:43-51 |
| write | `Create or overwrite files` | `Use write only for new files or complete rewrites.` | CA/core/tools/write.ts:16-19 |
| grep | `Search file contents for patterns (respects .gitignore)` | none | CA/core/tools/grep.ts:35-38 |
| find | `Find files by glob pattern (respects .gitignore)` | none | CA/core/tools/find.ts:34-37 |
| ls | `List directory contents` | none | CA/core/tools/ls.ts:16-19 |
| powershell | `Execute PowerShell commands` | same PI_* bullet as bash | CA/core/tools/powershell.ts:18-21 |

Edit guidelines (edit.ts:46-49):
```
Use edit for precise changes (edits[].oldText must match exactly)
When changing multiple separate locations in one file, use one edit call with multiple entries in edits[] instead of multiple edit calls
Each edits[].oldText is matched against the original file, not after earlier edits are applied. Do not emit overlapping or nested edits. Merge nearby changes into one edit.
Keep edits[].oldText as small as possible while still being unique in the file. Do not pad with large unchanged regions.
```

The `PI_*` vars mentioned in the bash guideline are `PI_SESSION_ID`, `PI_SESSION_FILE`, `PI_PROVIDER`, `PI_MODEL` and `PI_REASONING_LEVEL`. They are injected into the bash child env when `exposeSessionEnvironment` is set (CA/core/tools/bash.ts:194-208).

**docs** (system-prompt.ts:153-160, verbatim template; the paths come from config.ts):
```
Pi documentation (read only when the user asks about pi itself, its SDK, extensions, themes, skills, or TUI):
- Main documentation: ${getReadmePath()}
- Additional docs: ${getDocsPath()}
- Examples: ${getExamplesPath()} (extensions, custom tools, SDK)
- When reading pi docs or examples, resolve docs/... under Additional docs and examples/... under Examples, not the current working directory
- When asked about: extensions (docs/extensions.md, examples/extensions/), themes (docs/themes.md), skills (docs/skills.md), prompt templates (docs/prompt-templates.md), TUI components (docs/tui.md), keybindings (docs/keybindings.md), SDK integrations (docs/sdk.md), custom providers (docs/custom-provider.md), adding models (docs/models.md), pi packages (docs/packages.md), environment variables (docs/environment-variables.md), MCP servers (docs/mcp.md), codemode scripts and non-LLM models such as classifiers and image models (docs/codemode.md)
- When working on pi topics, read the docs and examples, and follow .md cross-references before implementing
- Always read pi .md files completely and follow links to related docs (e.g., tui.md for TUI API details)
```
The paths are `resolve(<packageDir>/README.md)`, `<packageDir>/docs` and `<packageDir>/examples` (CA/config.ts:444-456). `packageDir` is `$PI_PACKAGE_DIR`, or the Bun binary's dir, or the nearest package.json upward (config.ts:393-405, 376-391).

**addendum** (classic only): the raw APPEND_SYSTEM text (system-prompt.ts:163).

**project_context** (system-prompt.ts:72-79, 164). Present only if there is at least one file:
```
Project-specific instructions and guidelines:

<project_instructions path="${path}">
${content}
</project_instructions>

<project_instructions path="...">
...
</project_instructions>
```
The parts are joined with `"\n\n"`. Content is raw, with the BOM stripped. **No size limit or truncation exists anywhere.**

**skills**: see 2.4. The section is present only if `read` or `bash` is selected (`read` preferred) and at least one model-visible skill exists (system-prompt.ts:165-169). The text is `.trim()`'d.

**cwd** (system-prompt.ts:170): `cwd.replace(/\\/g, "/")`. It is always present and has no label text, so the result is `<cwd>\n/abs/path\n</cwd>`.

**Example rendering**: durable server, tools read/write/edit/bash, one AGENTS.md, no skills. Sections are joined by a blank line.
```
You are an expert coding assistant operating inside pi, ...

<tools>
- read: Read file contents
- write: Create or overwrite files
- edit: Make precise file edits with exact text replacement, including multiple disjoint edits in one call
- bash: Execute bash commands (ls, grep, find, etc.)

In addition to the tools above, you may have access to other custom tools depending on the project.
</tools>

<rules>
- Use bash for file operations like ls, rg, find
- Use read to examine files instead of cat or sed.
- Use write only for new files or complete rewrites.
- Use edit for precise changes (edits[].oldText must match exactly)
- (3 more edit bullets)
- You can inspect PI_* environment variables for current model and session details.
- Be concise in your responses
- Show file paths clearly when working with files
</rules>

<docs>
Pi documentation (...)
</docs>

<project_context>
Project-specific instructions and guidelines:

<project_instructions path="/repo/AGENTS.md">
...
</project_instructions>
</project_context>

<cwd>
/repo
</cwd>
```

### 2.3 Project instruction files (AGENTS.md etc.)

`loadProjectContextFiles({cwd, agentDir})` (CA/core/resource-loader.ts:232-270).

- **File names**: per directory, first match wins, in this order: `AGENTS.override.md`, `AGENTS.md`, `AGENTS.MD`, `CLAUDE.md`, `CLAUDE.MD` (resource-loader.ts:185).
  - At most **one file per directory**. A candidate that exists but is not a regular file is skipped.
  - A read error logs a yellow warning and moves on to the next candidate (resource-loader.ts:184-203).
  - Content is `stripBom(readFileSync(...,"utf-8"))`.
- **Global file**: the same candidate list in `agentDir` (default `~/.pi/agent/`). It is emitted **first** (resource-loader.ts:242-246).
- **Ancestor walk**: starts at `resolvePath(cwd)` and walks `dirname()` up to the filesystem root. The stop condition is `parentDir === currentDir` (resource-loader.ts:253-265).
  - There is **no stop at the git root or $HOME**.
  - Files are `unshift`ed, so the final order is **global, then root-most ancestor, …, then cwd** (outermost first, most specific last).
- **Dedupe**: by exact path string (`seenPaths`), so agentDir's file is not repeated if it is also an ancestor (resource-loader.ts:240, 257).
- **Worktree shadowing**: `findShadowedContextFile()` (resource-loader.ts:214-230). If cwd sits in a git linked worktree nested *inside* its main repo checkout, the main repo's context file with the same basename is skipped, so it is not applied twice.
  - Paths are compared after `canonicalizePath` (realpath).
  - Bare layouts, sibling worktrees and submodules are excluded.
- **Trust gating: none.** AGENTS.md/CLAUDE.md are loaded regardless of project trust. Trust gates only `.pi/SYSTEM.md` and `.pi/APPEND_SYSTEM.md` (below), project settings, `.pi/` resources and `.agents/skills`.
- **Disable**: `--no-context-files` / `-nc` (CA/cli/args.ts:207) sets loader `noContextFiles` (resource-loader.ts:634-641). An `agentsFilesOverride` hook exists.
- **SYSTEM.md / APPEND_SYSTEM.md** (resource-loader.ts:1201-1227):
  - Lookup order: `<cwd>/.pi/SYSTEM.md` (only if `settingsManager.isProjectTrusted()`), else `<agentDir>/SYSTEM.md`. APPEND_SYSTEM.md uses the same lookup.
  - CLI `--system-prompt` / `--append-system-prompt` override discovery (args.ts:120-122). Each value is used as text, or read as a file if the path exists (`resolvePromptInput`, resource-loader.ts:167-182).
  - The durable server ignores both.

### 2.4 Skills

**Format** (CA/core/skills.ts):
- YAML frontmatter is parsed by `parseFrontmatter`. The text must start with `---`, the block ends at the first `\n---`, CRLF is normalized, and the body is trimmed (CA/utils/frontmatter.ts:11-38).
- Fields: `name`, `description`, `disable-model-invocation` (bool) (skills.ts:67-72).
- **Name** is the frontmatter `name`, or the parent dir name if absent (skills.ts:320-321). It is validated (skills.ts:92-112):
  - at most 64 chars;
  - `^[a-z0-9-]+$`;
  - no leading or trailing `-`;
  - no `--`.
  Violations are **warnings only**; the skill still loads.
- **Description** is required and must be non-blank. It is at most 1024 chars (over the limit gives a warning, but the skill still loads). A missing or blank description means the skill is **not loaded** (skills.ts:117-127, 304-332).
  - A non-SKILL.md `.md` file without a description is silently ignored.
  - A YAML parse error warns only for `SKILL.md` (skills.ts:293-302).
- Skill record: `{name, description, filePath, baseDir: dirname(file), sourceInfo, disableModelInvocation}` (skills.ts:334-343).

**Directory scan** `loadSkillsFromDirInternal` (skills.ts:173-275):
- If a dir contains `SKILL.md`, that is the skill and the scan **does not recurse**.
- Otherwise it recurses into subdirectories, skipping dot-entries and `node_modules` and following symlinks.
- Root-level `*.md` files are skills only at the scan root (`includeRootFiles`).
- `.gitignore`, `.ignore` and `.fdignore` are honored, with patterns prefixed by their relative dir (skills.ts:16, 24-65).

**Discovery: classic loader path** (CA/core/package-manager.ts:920-989, 2433-2577; then `loadSkills(..., includeDefaults:false)` at CA/core/resource-loader.ts:828-849). Accumulation order; first registration of a name wins:
1. Packages from settings `packages`. Project packages come first, then global; project wins dedupe (package-manager.ts:925-936).
2. Explicit `skills` entries from settings: project entries are resolved against `<cwd>/.pi`, then global entries against agentDir (package-manager.ts:941-967).
3. Auto-discovered, in this order:
   - `<cwd>/.pi/skills` ("pi" mode, root `.md` files allowed). **Trusted only.**
   - `.agents/skills` in cwd and **each ancestor up to the git repo root**, or to the filesystem root if there is no `.git` ("agents" mode). **Trusted only.** `~/.agents/skills` is excluded here (package-manager.ts:468-487, 2458-2512).
   - `<agentDir>/skills` ("pi" mode) (:2541-2547).
   - `~/.agents/skills` ("agents" mode) (:2549-2561).
   - In "agents" mode, loose `.md` files are taken only below the root, never at it (package-manager.ts:428-432).
4. CLI `--skill <path>` (args.ts:184) is merged ahead of these (resource-loader.ts:587-589). `--no-skills` / `-ns` keeps only CLI skills.
5. Settings entries support `+`/`-`/`!` override patterns (`isEnabledByOverrides`, package-manager.ts:718).

**There is no `.claude/skills` discovery** anywhere.

**Discovery: durable server** (prompt.ts:34): `loadSkills({cwd, agentDir, skillPaths: settings.getSkillPaths(), includeDefaults:true})`.
- That is `<agentDir>/skills`, then `<cwd>/.pi/skills` (**not trust-gated**; skills.ts:452-455), then the merged settings `skills` array resolved **against cwd** (skills.ts:477-503).
- It does **not** scan `.agents/skills` or packages. This is a behavioural gap a port should decide on.

**Collisions** (skills.ts:421-450):
- The same realpath is skipped silently (symlink dedupe).
- The same name gives a diagnostic `{type:"collision", message:\`name "${name}" collision\`, collision:{resourceType:"skill", name, winnerPath, loserPath}}`. **First wins.**
- Other diagnostics are warnings: `skill path does not exist`, `skill path is not a markdown file`, plus the validation messages.

**Prompt listing** `formatSkillsForPrompt(skills, "read"|"bash")` (skills.ts:355-383):
- Skills with `disable-model-invocation: true` are excluded from the listing.
- XML-escaping covers `& < > " '`.
- Exact output before the trim:
```
\n\nThe following skills provide specialized instructions for specific tasks.
Use the read tool to load a skill's file when the task matches its description.     <- or: "Use bash to load a skill's file when the task matches its description."
When a skill file references a relative path, resolve it against the skill directory (parent of SKILL.md / dirname of the path) and use that absolute path in tool commands.

<available_skills>
  <skill>
    <name>NAME</name>
    <description>DESC</description>
    <location>/abs/path/SKILL.md</location>
  </skill>
</available_skills>
```

**Explicit invocation**: `/skill:name args` (CA/core/agent-session.ts:2100-2124). It is enabled by the setting `enableSkillCommands`, default true, and the command is registered as `skill:${name}` (:3330). The user message is replaced by:
```
<skill name="${name}" location="${filePath}">\nReferences are relative to ${baseDir}.\n\n${body-without-frontmatter, trimmed}\n</skill>
```
followed by `\n\n${args}` if there are args. The parser regex is at agent-session.ts:172. Expansion happens after extension `input` handlers and before prompt-template expansion (:1958-1963).

### 2.5 Prompt templates

CA/core/prompt-templates.ts.

- **Discovery** (classic): package-manager auto dirs are `<cwd>/.pi/prompts` (trusted only) and `<agentDir>/prompts`, plus settings `prompts` entries, packages, and CLI `--prompt-template` (args.ts:187). `--no-prompt-templates` / `-np` disables discovery.
  - Each dir is scanned **non-recursively** for `*.md`, symlinks followed (prompt-templates.ts:159-198).
  - The legacy `includeDefaults` order is global then project (prompt-templates.ts:268-271).
  - Dedupe is by name, first wins, with diagnostic `name "/${name}" collision` (resource-loader.ts:1148-1172).
- **Format**:
  - The name is the file basename without `.md`.
  - Frontmatter `description`. If absent, the first non-blank body line is used, truncated to 60 chars plus `...`.
  - Optional `argument-hint`.
  - `content` is the body after the frontmatter (prompt-templates.ts:105-154).
- **Invocation**: text `/name args`, regex `^\/([^\s]+)(?:\s+([\s\S]*))?$` (prompt-templates.ts:304-320).
  - Args are split bash-style with `'`/`"` quotes. There are no escapes (parseCommandArgs, :25-56).
  - Substitution regex (:75) handles:
    - `$1`, `$2`, … (missing gives `""`);
    - `$@` and `$ARGUMENTS` (all args joined with a space);
    - `${N:-default}`, `${@:-default}`, `${ARGUMENTS:-default}` (default when missing or empty);
    - `${@:N}` and `${@:N:L}` (1-based slice; 0 is treated as 1).
  - Substitution is not recursive. An unknown template name passes through unchanged.

## 7. Settings, config files, env vars, data dir layout

### 7.1 config.ts path helpers

- `APP_NAME = piConfig.name || "pi"` and `CONFIG_DIR_NAME = piConfig.configDir || ".pi"`, both from package.json `piConfig` (CA/config.ts:577-582).
- Env var names are derived from APP_NAME: `ENV_AGENT_DIR = ${APP}_CODING_AGENT_DIR` (`PI_CODING_AGENT_DIR`) and `ENV_SESSION_DIR = ${APP}_CODING_AGENT_SESSION_DIR` (config.ts:585-586).

| helper | value | ref |
|---|---|---|
| `getAgentDir()` | `$PI_CODING_AGENT_DIR` (tilde-expanded via normalizePath) else `~/.pi/agent` | config.ts:605-611 |
| `getPackageDir()` | `$PI_PACKAGE_DIR`, or Bun-binary dir, or nearest package.json (skipping `dist/`) | :393-405 |
| `getThemesDir()` | `<pkg>/theme` (bun) or `<pkg>/{src,dist}/modes/interactive/theme` | :413-421 |
| `getExportTemplateDir()` | `<pkg>/export-html` or `<pkg>/{src,dist}/core/export-html` | :429-436 |
| `getPackageJsonPath()` / `getReadmePath()` / `getDocsPath()` / `getExamplesPath()` / `getChangelogPath()` | `<pkg>/package.json`, `README.md`, `docs`, `examples`, `CHANGELOG.md` | :439-461 |
| `getInteractiveAssetsDir()` | `<pkg>/assets` or `<pkg>/{src,dist}/modes/interactive/assets` | :469-476 |
| `getCustomThemesDir()` | `<agent>/themes` | :614-616 |
| `getModelsPath()` | `<agent>/models.json` | :619-621 |
| `getAuthPath()` | `<agent>/auth.json` | :624-626 |
| `getSettingsPath()` | `<agent>/settings.json` | :629-631 |
| `getToolsDir()` | `<agent>/tools` | :634-636 |
| `getBinDir()` | `<agent>/bin` (managed fd, rg) | :639-641 |
| `getPromptsDir()` | `<agent>/prompts` | :644-646 |
| `getSessionsDir()` | `<agent>/sessions` | :649-651 |
| `getDebugLogPath()` | `<agent>/pi-debug.log` | :654-656 |
| `getShareViewerUrl(id)` | `${PI_SHARE_VIEWER_URL \|\| "https://pi.dev/session/"}#${id}` | :592-598 |

### 7.2 Agent dir layout (`~/.pi/agent/`, observed joins)

- **Core files**:
  - `settings.json`;
  - `auth.json` (mode 0600, dir 0700; CA/core/auth-storage.ts:25, 56-66);
  - `models.json`;
  - `models-store.json` (CA/core/models-store.ts:52);
  - `trust.json` (CA/core/trust-manager.ts:214);
  - `keybindings.json` (CA/core/keybindings.ts:380);
  - `crashes.json` (CA/core/crash-log.ts:22);
  - `pi-debug.log`.
- **Prompt files**: `AGENTS.md` / `CLAUDE.md` (global context), `SYSTEM.md`, `APPEND_SYSTEM.md`.
- **Resource dirs**: `skills/`, `prompts/`, `themes/`, `extensions/`, `tools/`, `bin/`.
- **Package dirs**: `npm/` (+`node_modules`) and `git/` (package-manager.ts:2093, 2134, 2174).
- **MCP**: `mcp.json`, `mcp-auth.json`, `mcp.log` (CA/extensions/mcp/oauth.ts:144, mcp/index.ts:351).
- **Classic sessions**: `sessions/--<cwd with leading / dropped and [/\\:] replaced by "-">--/<fileTimestamp>_<sessionId>.jsonl` (CA/core/session-manager.ts:589-594, 1080). Overridden by `--session-dir`, then `$PI_CODING_AGENT_SESSION_DIR`, then setting `sessionDir` (CA/main.ts:688-692).
- **Legacy**: `oauth.json` is migrated (CA/migrations.ts:24).
- **Project-side** (`<cwd>/.pi/`): `settings.json`, `mcp.json`, `extensions/`, `skills/`, `prompts/`, `themes/`, `SYSTEM.md`, `APPEND_SYSTEM.md`, `npm/`, `git/`. Any of the first eight existing makes the project require trust (trust-manager.ts:30-39, 186-208).

### 7.3 Experimental durable / server data layout

- **Durable CLI sessions** (CA/experimental/durable/sessions.ts:18-56):
  - Path: `<agentDir>/experimental/durable-sessions/<sha256(realpath(cwd)) hex[0..24]>/<13-digit zero-padded ms>-<uuidv4>/session.sqlite`.
  - "Continue" picks the lexically last dir matching `^\d{13}-[0-9a-f-]{36}$`.
  - The session dir is locked with `proper-lockfile.lock(directory, {realpath:false, retries:{retries:12,min/maxTimeout:1000}})`, which creates the sibling `<dir>.lock`; the default stale time is 10 s. On failure: `Session is already open in another process: <dir>`.
  - The `vacation` variant is identical under `experimental/vacation-sessions/` (CA/experimental/vacation/sessions.ts:18-25).
- **Server dir** (`resolveServerDirectory`, CA/experimental/server.ts:56-58): `--dir`, else `$PI_SERVER_DIR`, else `~/.pi/server`. **Note:** the code hard-codes `.pi`, not agentDir.
  - The dir is created with mode 0700 and must be owned by the uid (:60-69).
  - Contents:
    - `default-server-id` (UUIDv4, 0600, created with `wx`; :98, 113-131);
    - lock files `launcher-<id>` (stale 30 s, update 10 s; :135-145) and `activation-<id>` (:222-230);
    - sockets `<serverId>.sock` (public; server/src/transports/unix/address.ts:4-9), `control-<id>.sock` and `server-<id>-<12hex nonce>.sock` (server.ts:621-624);
    - `plugin-packages-<id>.json`, `plugin-builds/<id>/<name>-plugin-<sha256[0..12]>/`, and `session-plugin-packages-<id>-<sha256(sessionPath)[0..24]>.json` (CA/experimental/plugins/package.ts:26, 75, 160-170).
  - `$PI_SERVER_ID` selects the server id (server.ts:54, 541, 726; client-runtime.ts:86).
- **Server session catalog** (CA/experimental/session-catalog.ts):
  - Root: `resolveSessionDirectory()` gives `--session-dir`, else `<agentDir>/experimental/sessions` (server.ts:91-93).
  - Each session is `<root>/<id>/meta.json` + `session.sqlite`.
    - The id regex is `^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$`; the default id is a UUIDv4.
    - `meta.json` is `{"createdAt":<ms>,"cwd":"<abs>"}`, tab-indented with a trailing newline (:58-78).
    - Entries with invalid meta are skipped (:28-55).
  - The worker locks `<root>/<id>` with proper-lockfile (`realpath:true, stale:2000, update:1000`, retries for 8 s; CA/experimental/session-worker.ts:518-523).
  - Session cwd must be an existing absolute directory and is realpath'd (server.ts:76-89).

### 7.4 settings.json: locations and merge

- **Locations:**
  - global: `<agentDir>/settings.json`;
  - project: `<cwd>/.pi/settings.json` (CA/core/settings-manager.ts:297-302, 417-429).
- **Locking:** `proper-lockfile.lockSync` with 10 attempts × 20 ms busy-wait. The file and dir are only created when writing (:304-359).
- **Trust:** project settings are read only if `projectTrusted`; otherwise they become `{}` (:473-476, 582-605). The default `projectTrusted` is true in `create()`, but the main path runs `resolveProjectTrusted` (see 7.7).
- **Merge** `deepMergeSettings(global, project)` (:191-254):
  - Plain objects merge recursively.
  - Arrays and scalars from the project **replace** global values; `undefined` is skipped.
  - Exception: `defaultTools`. If the overriding list contains only `+name` / `-name` entries, it is appended. `resolveDefaultTools` then applies plain names (replacing `["read","bash","edit","write"]`), followed by `+` adds and `-` removes in order (:217-247).
- **Global-only reads**: `defaultProjectTrust`, `cacheWarming`, `deviceId` and `httpProxy` (`getGlobalSettings().httpProxy`, CA/main.ts:594, 871).
- **Writes**: setters write to global unless a project setter is used.
- **Migrations** on load (:504-563):
  - `queueMode` becomes `steeringMode`.
  - `websockets` (bool) becomes `transport` `"websocket"` or `"sse"`.
  - The old `skills:{enableSkillCommands, customDirectories}` object becomes `enableSkillCommands` and a `skills` array.
  - `retry.maxDelayMs` becomes `retry.provider.maxRetryDelayMs`.
- **JSON**: plain JSON with the BOM stripped. Comments are not stripped, unlike models.json.

| key | type | default (getter ref) |
|---|---|---|
| lastChangelogVersion | string | — |
| defaultProvider / defaultModel | string | — |
| defaultThinkingLevel | ThinkingLevel | — |
| modelThinkingLevels | Record<"provider/modelId", ThinkingLevel> | {} |
| transport | "auto"\|"sse"\|"websocket"… | "auto" (:904-905) |
| steeringMode / followUpMode | "all"\|"one-at-a-time" | "one-at-a-time" (:830-842) |
| theme | string (a value containing `/` is ignored by getTheme) | — (:850-859) |
| compaction.enabled / reserveTokens / keepRecentTokens / modelOverrides{"prov/id":{reserveTokens,keepRecentTokens}} | bool/num/num/record | true / 16384 / 20000 (:23-33, 914-960) |
| branchSummary.reserveTokens / skipPrompt | num/bool | 16384 / false (:976-984) |
| retry.enabled / maxRetries / baseDelayMs / maxAgentDelayMs | bool/num | true / 3 / 2000 / 60000 (:987-1006; ai/src/utils/retry.ts:121) |
| retry.provider.timeoutMs / maxRetries / maxRetryDelayMs | num | — / — / 60000 (:1034-1040) |
| hideThinkingBlock | bool | false |
| showCacheMissNotices | bool | false |
| externalEditor | string | then `$VISUAL`, then `$EDITOR` (:1054-1062) |
| shellPath | string (~ expanded) | — |
| quietStartup | bool\|"header" | false |
| defaultProjectTrust (global only) | "ask"\|"always"\|"never" | "ask" (:1100-1103) |
| shellCommandPrefix | string | — |
| npmCommand | string[] argv | — |
| collapseChangelog | bool | false |
| enableInstallTelemetry | bool | true (`$PI_TELEMETRY` overrides; CA/core/telemetry.ts:10-12) |
| enableAnalytics / trackingId | bool / uuid | false / generated on enable |
| deviceId (global only) | uuid | created lazily (:1175-1181) |
| packages | (string \| {source, autoload?, extensions?, skills?, prompts?, themes?})[] | [] (:122-131) |
| extensions / skills / prompts / themes | string[] (paths or `+`/`-`/`!` patterns) | [] |
| enableSkillCommands | bool | true (:1264-1265) |
| terminal.showImages / imageWidthCells / clearOnShrink / showTerminalProgress / hyperlinks / images / trueColor | bool / num / bool / bool / bool\|"auto" / "kitty"\|"iterm2"\|"auto"\|false / bool\|"auto" | true / 60 / `$PI_CLEAR_ON_SHRINK==="1"` / false / auto / auto / auto (:57-65, 1278-1343) |
| images.autoResize / blockImages | bool | true / false (:1403-1417) |
| enabledModels | string[] patterns | — |
| defaultTools | string[] | ["read","bash","edit","write"] (:215) |
| doubleEscapeAction | "fork"\|"tree"\|"none" | "tree" |
| treeFilterMode | "default"\|"no-tools"\|"user-only"\|"labeled-only"\|"all" | "default" |
| thinkingBudgets.{minimal,low,medium,high} | num | — |
| editorPaddingX | num | 0 |
| outputPad | 0\|1 | 1 |
| autocompleteMaxVisible | num | 5 |
| showHardwareCursor | bool | `$PI_HARDWARE_CURSOR==="1"` |
| markdown.codeBlockIndent / mermaid | string / "off"\|"final"\|"streaming" | "  " / "streaming" |
| warnings.anthropicExtraUsage | bool | true |
| codemode.mode / inlineBudget | "on"\|"only" / num | "on" / 3000 (CA/extensions/codemode/index.ts:23-27) |
| sessionDir | string (~ expanded) | — |
| httpProxy (global) | URL | sets `HTTP_PROXY` / `HTTPS_PROXY` if unset (CA/core/http-dispatcher.ts:45-50) |
| httpIdleTimeoutMs | number, or "disabled" (=0) | 300000 (http-dispatcher.ts:4, 19-35) |
| cacheWarming (global) | "off"\|"streaming"\|"idle" | "streaming" (:1023-1026) |
| websocketConnectTimeoutMs | num | — |
| tuiMode | "fullscreen"\|"regular" | "fullscreen" |
| fullscreenExitOutput | "transcript"\|"resume-hint" | "transcript" |
| fullscreenScrollbar | "auto"\|"always"\|"hidden" | "auto" |
| fullscreenCopyOnSelect | bool | true |
| fullscreenWheelScrollLines | 1-100 \| "auto" | "auto" |

The durable harness reads only these settings: `stream` (provider retry/idle timeout; idle 0 maps to 2147483647), `compaction`, `retry`, `steeringMode`, `followUpMode` and `skills` (CA/experimental/durable/harness-setup.ts:25-49; prompt.ts:34).

### 7.5 auth.json and models.json

- **auth.json** (CA/core/auth-storage.ts):
  - Format: `Record<providerId, Credential>`, where `Credential` is one of (ai/src/auth/types.ts:17-37):
    - `{type:"api_key", key?:string, env?:Record<string,string>}`
    - `{type:"oauth", refresh, access, expires:number, ...extra}`
  - Locking: sync lock is 10×20 ms; async lock has stale 30 s, exponential backoff to 2 s, and abort support (:69-200).
  - An empty file is created as `{}`.
  - `key` resolution (CA/core/resolve-config-value.ts:80-140):
    - A leading `!` means a shell command; its stdout is cached for the process lifetime.
    - Otherwise the value is a template with `$NAME` / `${NAME}` env refs, looked up in credential `env` first and then process.env.
    - `$$` and `$!` are escapes.
    - The whole value is unresolved if any referenced var is missing.
- **models.json** (CA/core/model-config.ts:242-333):
  - Shape: `{providers:{[id]:{name?, baseUrl?, apiKey?, api?, oauth?:"radius", headers?, compat?, authHeader?, models?:ModelDefinition[], modelOverrides?:Record<modelId,ModelOverride>}}}`.
  - It is JSON with comments: `stripJsonComments`, BOM stripped.
  - Validation is a typebox schema. On error the result is an empty config plus the message `Invalid models.json schema:\n  - path: msg`.
  - ModelDefinition fields: `id` (required), `name`, `api`, `baseUrl`, `reasoning`, `thinkingLevelMap`, `input` ("text" or "image"), `inputLimits`, `cost{input,output,cacheRead,cacheWrite,tiers[]}`, `promptCache{short,long}`, `contextWindow`, `maxTokens`, `samplingParams`, `samplingParamsByThinkingLevel`, `headers`, `compat` (:199-216).
- **models-store.json**: `Record<providerId,{models, lastModified?, checkedAt?, etag?}>` (ai/src/models-store.ts:3-15). It is a cache of remote catalogs and uses the same lock backend.
- **trust.json**: `Record<canonicalAbsPath, true|false|null>`.
  - Written with sorted keys and a trailing newline.
  - The lock is `trust.json.lock`.
  - The nearest ancestor entry wins (trust-manager.ts:45-59, 99-136, 210-229).

### 7.6 Environment variables read

Variables are grouped by package. "CA" is coding-agent/src.

- **Paths and app**:
  - `PI_CODING_AGENT_DIR`: agent dir (config.ts:606). If set, it also suppresses first-run setup (CA/cli/startup-ui.ts:144).
  - `PI_CODING_AGENT_SESSION_DIR`: classic session dir (main.ts:688).
  - `PI_PACKAGE_DIR`: package/docs root (config.ts:395).
  - `PI_SHARE_VIEWER_URL` (config.ts:596).
  - `PNPM_HOME`: self-update (config.ts:136).
  - `PI_MANAGED_INSTALL_ROOT` and `PI_INSTALLER_API_BASE`: managed installer (CA/package-manager-cli.ts:55, 189).
- **Behaviour flags**:
  - `PI_OFFLINE`: 1, true or yes. It disables network package, version and model refresh. `--offline` sets it along with `PI_SKIP_VERSION_CHECK=1` (main.ts:576-579; package-manager.ts:55; model-runtime.ts:239; tools-manager.ts:15; version-check.ts:55).
  - `PI_SKIP_VERSION_CHECK` (version-check.ts:98).
  - `PI_EXPERIMENTAL=1`: experimental features (CA/core/experimental.ts:2).
  - `PI_TELEMETRY` (telemetry.ts:10).
  - `PI_TIMING=1` (timings.ts:6).
  - `PI_STARTUP_BENCHMARK`: interactive only (main.ts:931).
  - `PI_CLEAR_ON_SHRINK` and `PI_HARDWARE_CURSOR` (settings fallbacks).
  - `PI_RADIUS_GATEWAY` (CA/core/radius.ts:4-11).
  - `LLAMA_BASE_URL`: llama extension (CA/extensions/llama/provider.ts:163).
- **Set by pi**:
  - `PI_CODING_AGENT=true` and `AI_AGENT=pi` (CA/cli/setup.ts:6-7, rpc-entry.ts:7-8).
  - `HTTP_PROXY` and `HTTPS_PROXY` from settings.
  - The bash child receives `PI_SESSION_ID`, `PI_SESSION_FILE`, `PI_PROVIDER`, `PI_MODEL` and `PI_REASONING_LEVEL` (bash.ts:194-208).
- **Experimental server**:
  - `PI_SERVER_DIR` and `PI_SERVER_ID` (server.ts:53-58).
  - `__PI_INTERNAL_SPAWN`: role coordinator, server or session-worker, consumed and then deleted (CA/experimental/process.ts:6-31).
  - `PI_SESSION_WORKER_CONTROL_ADDRESS`, `PI_SESSION_WORKER_CONTROL_TOKEN`, `PI_SESSION_WORKER_SESSION_KEY_BASE64` and `PI_SESSION_WORKER_PEER_ID` (session-worker.ts:65-68, 335-339).
  - `__PI_SESSION_WORKER_INITIAL_DEMAND_GRACE_MS` (default 10000) and `__PI_SESSION_WORKER_ORPHAN_DEMAND_GRACE_MS` (default 30000) (:305-311, 463-467).
- **System**:
  - `HOME` and `USERPROFILE`: ~ display and trust (trust-manager.ts:187; package-manager.ts:228).
  - `VISUAL` and `EDITOR`.
  - `SHELL`, `TMUX`, `SSH_*`, `CI` (bug report; interactive-mode.ts:1274).
  - `DISPLAY`, `WAYLAND_DISPLAY`, `TERMUX_VERSION` (clipboard).
  - `WSL_DISTRO_NAME` and `WSL_INTEROP` (footer).
  - `ProgramFiles`, `ProgramFiles(x86)`, `SystemRoot` and `WINDIR` (Windows shell lookup; CA/utils/shell.ts:79-192; durable/src/env/node.ts:226-278).
  - The PATH key (shell.ts:141).
  - On Bun only, `/proc/self/environ` is used as a fallback when process.env is empty (CA/bun/restore-sandbox-env.ts:30; ai/src/utils/provider-env.ts:15-52).
- **ai/src** (all via `getProviderEnvValue(name, env)`; precedence is credential `env`, then process.env, then the Bun fallback):
  - Provider API keys (ai/src/env-api-keys.ts:73-127):
    - anthropic: `ANTHROPIC_AUTH_TOKEN`, `ANTHROPIC_OAUTH_TOKEN`, `ANTHROPIC_API_KEY`.
    - `OPENAI_API_KEY`, `AZURE_OPENAI_API_KEY`, `NVIDIA_API_KEY`, `DEEPSEEK_API_KEY`.
    - `GEMINI_API_KEY` (google), `GOOGLE_CLOUD_API_KEY` (vertex).
    - `GROQ_API_KEY`, `CEREBRAS_API_KEY`, `XAI_API_KEY`, `TYPESAFE_API_KEY`, `RADIUS_API_KEY`, `OPENROUTER_API_KEY`, `AI_GATEWAY_API_KEY`.
    - `ZAI_API_KEY`, `ZAI_CODING_CN_API_KEY`, `MISTRAL_API_KEY`, `MINIMAX_API_KEY`, `MINIMAX_CN_API_KEY`, `MOONSHOT_API_KEY`, `HF_TOKEN`, `FIREWORKS_API_KEY`, `TOGETHER_API_KEY`, `BASETEN_API_KEY`, `OPENCODE_API_KEY`, `KIMI_API_KEY`, `META_API_KEY`, `CLOUDFLARE_API_KEY`.
    - `XIAOMI_API_KEY` and `XIAOMI_TOKEN_PLAN_{CN,AMS,SGP}_API_KEY`; `QWEN_TOKEN_PLAN_API_KEY` and `QWEN_TOKEN_PLAN_CN_API_KEY`; `ANT_LING_API_KEY`.
    - `COPILOT_GITHUB_TOKEN` (github-copilot).
  - Anthropic org/federation: `ANTHROPIC_FEDERATION_RULE_ID`, `ANTHROPIC_ORGANIZATION_ID`, `ANTHROPIC_SERVICE_ACCOUNT_ID`, `ANTHROPIC_IDENTITY_TOKEN_FILE`, `ANTHROPIC_WORKSPACE_ID` (:29-36).
  - Bedrock: `AWS_PROFILE`, `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_SESSION_TOKEN`, `AWS_BEARER_TOKEN_BEDROCK`, `AWS_CONTAINER_CREDENTIALS_{RELATIVE,FULL}_URI`, `AWS_WEB_IDENTITY_TOKEN_FILE`, `AWS_REGION`, `AWS_DEFAULT_REGION`, `AWS_BEDROCK_FORCE_CACHE`, `AWS_BEDROCK_FORCE_HTTP1`, `AWS_BEDROCK_SKIP_AUTH`.
  - Vertex: `GOOGLE_APPLICATION_CREDENTIALS` (else `~/.config/gcloud/application_default_credentials.json`), `GOOGLE_CLOUD_PROJECT` / `GCLOUD_PROJECT`, `GOOGLE_CLOUD_LOCATION`.
  - Azure: `AZURE_OPENAI_BASE_URL`, `AZURE_OPENAI_RESOURCE_NAME`, `AZURE_OPENAI_API_VERSION`, `AZURE_OPENAI_DEPLOYMENT_NAME_MAP`.
  - Cloudflare: `CLOUDFLARE_ACCOUNT_ID` and `CLOUDFLARE_GATEWAY_ID`.
  - `PI_CACHE_RETENTION=long`: prompt-cache TTL "long" (ai/src/api/anthropic-messages.ts:73, openai-*.ts, bedrock, pi-messages; CA/core/cache-warmer.ts:42).
  - `PI_OAUTH_CALLBACK_HOST`: default 127.0.0.1 (ai/src/auth/oauth/anthropic.ts:17 and openai/openrouter).
  - `KIMI_OAUTH_HOST` / `KIMI_CODE_OAUTH_HOST`.
  - Proxy: `{http,https}_proxy`, `all_proxy`, `no_proxy`, upper or lower case (ai/src/utils/node-http-proxy.ts:13-20, 75, 131).
- **agent/src**: no `process.env` reads.

### 7.7 Project trust

`resolveProjectTrusted` (CA/core/project-trust.ts:46-96) resolves in this order:
1. CLI override.
2. If `hasTrustRequiringProjectResources(cwd)` is false, the project is trusted. Trust-requiring resources are `.pi/{settings.json,mcp.json,extensions,skills,prompts,themes,SYSTEM.md,APPEND_SYSTEM.md}`, or any ancestor `.agents/skills` other than `~/.agents/skills`.
3. An extension `project_trust` event.
4. The nearest `trust.json` entry.
5. `defaultProjectTrust` set to always or never.
6. With no UI, untrusted.
7. Otherwise a prompt with these options:
   - `Trust`
   - `Trust parent folder (<p>)`
   - `Trust (this session only)`
   - `Do not trust`
   - `Do not trust (this session only)`

   The prompt text is `Trust project folder?\n<cwd>\n\nThis allows pi to load .pi settings and resources, install missing project packages, and execute project extensions.`

The loader runs a pre-trust extension pass with project settings forced off (CA/core/resource-loader.ts:497-517).

The experimental server and durable runtime call `SettingsManager.create(cwd)` with the default `projectTrusted: true` and no trust prompt (CA/experimental/durable/runtime.ts:130; session-worker.ts:780).
