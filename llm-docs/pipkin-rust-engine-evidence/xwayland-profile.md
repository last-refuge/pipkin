# Native profile inspection and Xwayland startup

2026-10-12. Runtime binary identity is the same as the successful
[dedicated-server desktop soak](server-allocator.md); no production change in this check.

## Inspection fixture replacement (E2E-36)

The historical `seed_a_profile_to_look_at` is a developer helper, not an end-user workflow
assertion. `scripts/qualify-native-desktop-soak.py` now provides a native equivalent: an
isolated retained desktop/engine profile, four conversations, streamed replies and large
real tool outputs, with exact automated durability assertions before inspection. Its
1,500-turn execution and guarded Wayland inspection are recorded in server-allocator.md.
The old helper's answered Pi extension questions are explicitly outside owner scope.

To create another inspectable native profile, run the documented desktop soak against a
fresh scratch directory. The harness prints its owned app PID, waits for guarded first-
conversation setup and `ROOT/go`, then leaves the profile/window open for inspection until
guarded Quit and `ROOT/finish`. It never deletes the retained profile. For later inspection,
start `pipkin --engine-server ROOT/endpoint --engine-data-dir ROOT/engine
--engine-config-dir ROOT/config`, then `pipkin --engine native --engine-attach ROOT/endpoint
--data-dir ROOT/profile --project ROOT/project`. Stop the owned server after quitting.
The local scripted provider is ephemeral, so reopening is for history inspection; start a
fresh harness for new scripted turns. This helper is evidence infrastructure, not itself
proof of the product workflows it makes visible.

## Xwayland observation (PLAT-X11)

A separate copy of the synthetic 1,500-turn profile launched with `WAYLAND_DISPLAY`
unset and `XDG_SESSION_TYPE=x11`. Hyprland reported `xwayland: true`, mapped and visible
on the current launch workspace (3). No workspace, scale, window geometry or compositor
configuration was changed. Four saved conversations, the selected transcript, composer
and sidebar rendered. Native server attachment and history loading worked.

Guarded Ctrl+K, text `Log performance stats`, Return and Ctrl+Q did **not** produce the
expected palette/action/Quit behavior: unrelated characters appeared in the composer,
and Quit timed out after 20 seconds. This is a recorded interaction failure, not a passed
shortcut test. Attribution between synthetic keyboard delivery and X11 event handling is
unresolved; a physical-keyboard test was not performed. The runner then terminated only
its owned app/server, and their absence was checked. No clean user-initiated Quit is claimed.
An earlier launch used server-only flags on the desktop CLI and was correctly rejected;
it did not reach the platform backend.

[Raw observation](../../fixtures/native-desktop-soak/2026-10-12-xwayland/) retains binary
identity, compositor fields, logs and screenshot hashes. Raw screenshots stay private
because compositor transparency includes unrelated desktop content. This establishes
startup/rendering only. Pure X11, full input/clipboard/IME/accessibility and additional
window managers remain unverified. The supported v1 scope remains Arch/Omarchy Wayland.
