#!/usr/bin/env bash
# Tests guard.sh against STUB hyprctl and wtype programs. Nothing here touches the real desktop:
# the stubs read canned JSON and only record what they were asked to send.
#   scripts/test-guard.sh
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
GUARD="${GUARD:-$HERE/guard.sh}"
T="$(mktemp -d)"; trap 'rm -rf "$T"' EXIT
mkdir -p "$T/bin"

# Stub hyprctl: answers from files the tests rewrite. `activewindow` may flip after N sends
# (FLIP_AFTER) to model focus being stolen mid-run.
cat >"$T/bin/hyprctl" <<'STUB'
#!/usr/bin/env bash
case "$*" in
  "activewindow -j")
    n=$(cat "$STUB_DIR/sent" 2>/dev/null | wc -l)
    if [[ -n "${FLIP_AFTER:-}" && "$n" -ge "$FLIP_AFTER" ]]; then cat "$STUB_DIR/stolen.json"; else cat "$STUB_DIR/active.json"; fi ;;
  "activeworkspace -j") echo '{"id": 2}' ;;
  "clients -j") cat "$STUB_DIR/clients.json" ;;
  *) echo "unexpected hyprctl $*" >&2; exit 9 ;;
esac
STUB
cat >"$T/bin/wtype" <<'STUB'
#!/usr/bin/env bash
echo "$*" >>"$STUB_DIR/sent"
STUB
chmod +x "$T/bin/hyprctl" "$T/bin/wtype"
export PATH="$T/bin:$PATH" STUB_DIR="$T" PIPKIN_GUARD_STATE="$T/pin" PIPKIN_GUARD_PIN_WAIT=1

win() { printf '{"class":"%s","address":"%s","pid":%s,"workspace":{"id":%s}}' "$1" "$2" "$3" "$4"; }
set_active() { win "$@" >"$T/active.json"; }
reset() { rm -f "$T/sent" "$T/pin"; unset FLIP_AFTER; set_active pipkin 0xaaa 100 2
          echo "[$(win pipkin 0xaaa 100 2),$(win pipkin 0xbbb 200 2)]" >"$T/clients.json"; win org.other 0xccc 300 2 >"$T/stolen.json"; }

pass=0; failed=0
ok()   { pass=$((pass+1)); echo "ok   $1"; }
bad()  { failed=$((failed+1)); echo "FAIL $1"; }
sent() { [[ -f "$T/sent" ]] && cat "$T/sent" || true; }
expect_refused() {   # name, command...
  local name="$1"; shift
  "$@" >/dev/null 2>&1; local rc=$?
  if [[ $rc -eq 3 && -z "$(sent)" ]]; then ok "$name"; else bad "$name (rc=$rc, sent='$(sent)')"; fi
}

reset; expect_refused "refuses with no pin" "$GUARD" key ctrl+k

reset; "$GUARD" pin 100 >/dev/null
"$GUARD" key ctrl+k >/dev/null 2>&1 && [[ "$(sent)" == "-M ctrl -k k -m ctrl" ]] && ok "sends a ctrl chord to the pinned window" || bad "ctrl chord (sent='$(sent)')"

reset; "$GUARD" pin 100 >/dev/null; "$GUARD" key Return >/dev/null 2>&1
[[ "$(sent)" == "-k Return" ]] && ok "sends a named key" || bad "named key (sent='$(sent)')"

reset; "$GUARD" pin 100 >/dev/null; "$GUARD" key ctrl+. >/dev/null 2>&1
[[ "$(sent)" == "-M ctrl -k period -m ctrl" ]] && ok "maps the Stop shortcut to the wtype keysym" || bad "period keysym (sent='$(sent)')"

reset; "$GUARD" pin 100 >/dev/null; "$GUARD" key ctrl+, >/dev/null 2>&1
[[ "$(sent)" == "-M ctrl -k comma -m ctrl" ]] && ok "maps the Settings shortcut to the wtype keysym" || bad "comma keysym (sent='$(sent)')"

reset; "$GUARD" pin 100 >/dev/null; "$GUARD" text "next conversation" >/dev/null 2>&1
[[ "$(sent)" == "-- next conversation" ]] && ok "types plain text" || bad "text (sent='$(sent)')"

reset; "$GUARD" pin 100 >/dev/null; "$GUARD" key ctrl+shift+p >/dev/null 2>&1
[[ "$(sent)" == "-M ctrl -M shift -k p -m ctrl -m shift" ]] && ok "stacks ctrl and shift" || bad "stacked mods (sent='$(sent)')"

reset; "$GUARD" pin 100 >/dev/null; set_active org.quickshell 0xddd 400 2
expect_refused "refuses when another app is active" "$GUARD" key Return

reset; "$GUARD" pin 100 >/dev/null; set_active org.other 0xaaa 100 2
expect_refused "refuses a different app even at the pinned address (class is checked too)" "$GUARD" key Return

reset; "$GUARD" pin 100 >/dev/null; set_active pipkin 0xbbb 200 2
expect_refused "refuses another pipkin instance (not the pinned window)" "$GUARD" key Return

reset; "$GUARD" pin 100 >/dev/null; set_active pipkin 0xaaa 200 2
expect_refused "refuses a recycled compositor address owned by another pid" "$GUARD" key Return

reset; printf '0xaaa\n' >"$T/pin"
expect_refused "legacy address-only pins require explicit repinning" "$GUARD" key Return

reset; "$GUARD" pin 100 >/dev/null; set_active pipkin 0xaaa 100 3
expect_refused "refuses when the window is not on the active workspace" "$GUARD" key Return

reset; "$GUARD" pin 100 >/dev/null; echo '{}' >"$T/active.json"
expect_refused "refuses with no active window" "$GUARD" key Return

reset; "$GUARD" pin 100 >/dev/null
for chord in super+q logo+Return alt+Down alt+ctrl+k ctrl+F5 F2 Super_L "ctrl+;" XF86AudioMute caps+a; do
  expect_refused "refuses disallowed chord '$chord'" "$GUARD" key "$chord"
done

reset; "$GUARD" pin 100 >/dev/null
expect_refused "one bad chord in a list sends nothing at all" "$GUARD" key ctrl+k Return super+q

reset; "$GUARD" pin 100 >/dev/null
expect_refused "refuses text with a newline" "$GUARD" text $'hi\nthere'
expect_refused "refuses text with a control character" "$GUARD" text $'hi\x1b[0m'
expect_refused "refuses empty text" "$GUARD" text ""
long="$(printf 'x%.0s' $(seq 1 201))"
expect_refused "refuses text over the limit" "$GUARD" text "$long"
keys=(); for i in $(seq 1 21); do keys+=(Tab); done
"$GUARD" key "${keys[@]}" >/dev/null 2>&1; rc=$?
[[ $rc -eq 2 && -z "$(sent)" ]] && ok "refuses more than 20 keys in one call" || bad "key limit (rc=$rc)"

# Focus stolen mid-sequence: the first key goes out, the re-check catches it, and the second
# key is NOT sent.
reset; "$GUARD" pin 100 >/dev/null; export FLIP_AFTER=1
"$GUARD" key ctrl+k Return >/dev/null 2>&1; rc=$?
if [[ $rc -eq 4 && "$(sent | wc -l)" -eq 1 ]]; then ok "stops after focus is lost mid-run (exit 4, no further keys)"
else bad "mid-run focus loss (rc=$rc, sent='$(sent)')"; fi
unset FLIP_AFTER

reset; "$GUARD" pin 100 >/dev/null; out="$("$GUARD" status 2>&1)"; rc=$?
[[ $rc -eq 0 && "$out" == *"OK"* && -z "$(sent)" ]] && ok "status reports OK and sends nothing" || bad "status ok (rc=$rc)"
set_active org.other 0xeee 500 2; out="$("$GUARD" status 2>&1)"; rc=$?
[[ $rc -eq 3 && "$out" == *REFUSED* && -z "$(sent)" ]] && ok "status reports REFUSED and sends nothing" || bad "status refused (rc=$rc)"

reset; "$GUARD" pin 999 >/dev/null 2>&1; rc=$?
[[ $rc -eq 3 && ! -s "$T/pin" ]] && ok "pin fails for a pid with no window" || bad "pin missing pid (rc=$rc)"
"$GUARD" pin notanumber >/dev/null 2>&1; rc=$?
[[ $rc -eq 2 ]] && ok "pin rejects a non-numeric pid" || bad "pin usage (rc=$rc)"

reset; "$GUARD" pin 200 >/dev/null; [[ "$(cat "$T/pin")" == $'0xbbb\n200' ]] && ok "pin records the address and owning pid" || bad "pin address and pid"
"$GUARD" unpin >/dev/null; [[ ! -e "$T/pin" ]] && ok "unpin removes the pin" || bad "unpin"

echo "---"; echo "passed $pass, failed $failed"
exit $(( failed > 0 ))
