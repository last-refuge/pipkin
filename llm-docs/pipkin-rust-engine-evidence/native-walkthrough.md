# Guarded native desktop walkthrough (native engine)

**Date:** 2026-10-11. **Build:** `cargo build --release -p pipkin-app` at `native-engine` `ae95240`.
**Machine:** Hyprland on Omarchy, DP-1 5120x2160 at scale 1.25.

**Profile:** a throwaway `--data-dir` in the session scratchpad. Its `engine-config/models.json`
defines one custom `openai-completions` provider at `http://127.0.0.1:18765/v1` (a local Python
stub that streams scripted replies). There were no credentials and no network provider. The
user's `~/.pi`, desktop config and other windows were not touched.

**Input:** only through `scripts/guard.sh`, pinned to the window I launched; every key was sent
with that window active. No mouse input; nothing moved, resized or refocused.

**Screenshots:** cropped to the Pipkin window. The first capture used the compositor's
geometry before the window was placed and included other desktop windows; it is kept only in
the session scratchpad, is not evidence, and is not shared.

## Observed

1. **First run, isolated profile** (`--engine native --project <dir>`): the welcome screen shows.
   "Get started" (Tab + Return) opens "Connect your AI":
   - **Claude Pro / Max is disabled** with the text "Claude sign-in is not available yet: Claude
     subscription sign-in is reserved for a later integration and is not implemented in this
     engine." There is no dead-end flow.
   - ChatGPT Plus / Pro is enabled.
   - No simulated content is shown anywhere.
2. **Relaunch.** The project was saved on the first launch, so the app opens straight into the
   workspace: "No conversations yet", the project selected, the Changes and Subagents tabs, and
   no demo marker.
3. **First conversation.** Ctrl+N, typed "Please write a file for me", then Return.
   - The stub received two requests: the prompt, then the turn after the tool result.
   - The engine ran the real `write` tool, and `hello.txt` exists in the project with the
     scripted content.
   - The transcript shows the user message, "Writing the file.", a "Writing a file hello.txt"
     card with a Diff action, and "Done: hello.txt was written."
   - Session changes shows `hello.txt +1 -0` with the diff.
   - The composer's model picker shows "Scripted (local stub)", the custom provider from
     `models.json`.
4. **Desktop SIGKILL.**
   - After `kill -9` of the desktop, no engine worker process remained. The worker exits when
     its stdin closes.
   - On relaunch, the conversation, transcript, tool card, diff, Files changed list and model
     selection all came back from the engine's store.

## Defect found (fixed afterwards)

The onboarding copy under ChatGPT said "ChatGPT sign-in uses Pi's openai-codex connection, not
an OpenAI API key." In native mode it should not mention Pi. It was corrected in the same
batch to engine-neutral copy ("uses your ChatGPT subscription (the openai-codex connection), not
an OpenAI API key"; `crates/pipkin-ui/src/shell/onboarding.rs`). The corrected text has not been
re-observed natively.

## Not covered by this walkthrough

- Real ChatGPT sign-in: needs an owner-authorized account.
- Stop and queue through the GUI: these are covered by the automated native e2e suite
  (`crates/pipkin-app/src/adapters/native/e2e.rs`), not observed here.
- Subagent inspector and extension questions through the GUI.
- IME, screen reader, display scale, suspend/resume and cold boot: unchanged, still unverified.
- macOS.
