#!/usr/bin/env bash
# guard.sh: the only sanctioned way to send synthetic keyboard input to Pipkin during native tests.
#
# Why it exists: input goes to whichever window has focus, and focus can change mid-run (another
# app once took it during an earlier session). So every send is preceded by a check that the
# window you LAUNCHED is the active window, and followed by a re-check.
#
#   guard.sh pin <pid>          record the window address of the Pipkin process you launched
#   guard.sh status             show active window vs the pin (sends nothing)
#   guard.sh key  <chord>...    e.g. ctrl+k  Return  alt-free keys only; checked before EACH key
#   guard.sh text "<string>"    type plain text (no control characters, at most 200 chars)
#   guard.sh unpin
#
# What it will not do, by design:
#   - no focus, move, resize or workspace changes: it only READS compositor state (hyprctl)
#   - no mouse or uinput
#   - no raw wtype passthrough, and no Super/Alt/Caps or function keys, because the compositor
#     may bind those and they would act on your desktop rather than on the app
#   - nothing without a pin, so your own Pipkin instance can never be the target
#
# Exit codes: 0 sent; 2 usage; 3 refused (nothing sent); 4 focus changed after a send (stop).
set -euo pipefail

APP_CLASS="${PIPKIN_APP_ID:-pipkin}"
STATE="${PIPKIN_GUARD_STATE:-${XDG_RUNTIME_DIR:-/tmp}/pipkin-guard.pin}"
LOG="${PIPKIN_GUARD_LOG:-$STATE.log}"
MAX_KEYS=20
MAX_TEXT=200
PIN_WAIT_SECONDS="${PIPKIN_GUARD_PIN_WAIT:-10}"

die()    { echo "guard: $*" >&2; exit "${2:-3}"; }
usage()  { sed -n '2,24p' "$0" | sed 's/^# \{0,1\}//' >&2; exit 2; }
log()    { printf '%s %s\n' "$(date -u +%FT%TZ)" "$*" >>"$LOG" 2>/dev/null || true; }
need()   { command -v "$1" >/dev/null 2>&1 || die "missing required tool: $1" 2; }

# Active window as "class address workspace-id pid", or empty if there is none.
active_window() {
  hyprctl activewindow -j 2>/dev/null | jq -r 'select(type=="object" and (.address // "") != "") |
    "\(.class)\t\(.address)\t\(.workspace.id)\t\(.pid)"' 2>/dev/null || true
}
active_workspace() { hyprctl activeworkspace -j 2>/dev/null | jq -r '.id // empty' 2>/dev/null || true; }

pinned_address() { [[ -s "$STATE" ]] || return 1; head -n1 "$STATE"; }

# Succeeds only if the pinned window is active, on the active workspace, with the right class.
# On failure prints why and returns 1. Never changes anything.
check() {
  local pin pin_pid line class address ws pid active_ws
  pin="$(pinned_address)" || { echo "no pin: run 'guard.sh pin <pid>' for the window you launched" >&2; return 1; }
  pin_pid="$(head -n2 "$STATE" | tail -n1)"
  [[ "$pin_pid" =~ ^[0-9]+$ ]] || { echo "pin has no process identity; explicitly pin the owned window again" >&2; return 1; }
  line="$(active_window)"
  [[ -n "$line" ]] || { echo "no active window" >&2; return 1; }
  IFS=$'\t' read -r class address ws pid <<<"$line"
  [[ "$class" == "$APP_CLASS" ]] || { echo "active window is '$class', not '$APP_CLASS'" >&2; return 1; }
  [[ "$address" == "$pin" ]] || { echo "active $APP_CLASS window $address is not the one you pinned ($pin)" >&2; return 1; }
  [[ "$pid" == "$pin_pid" ]] || { echo "pinned window address now belongs to another process" >&2; return 1; }
  active_ws="$(active_workspace)"
  [[ -n "$active_ws" && "$ws" == "$active_ws" ]] || { echo "pinned window is not on the active workspace" >&2; return 1; }
  return 0
}

cmd_pin() {
  local pid="${1:-}" address="" deadline
  [[ "$pid" =~ ^[0-9]+$ ]] || usage
  deadline=$((SECONDS + PIN_WAIT_SECONDS))
  while (( SECONDS <= deadline )); do
    address="$(hyprctl clients -j 2>/dev/null | jq -r --argjson pid "$pid" --arg class "$APP_CLASS" \
      '[.[] | select(.pid == $pid and .class == $class)] | first | .address // empty' 2>/dev/null || true)"
    [[ -n "$address" ]] && break
    sleep 0.25
  done
  [[ -n "$address" ]] || die "no '$APP_CLASS' window for pid $pid appeared within ${PIN_WAIT_SECONDS}s" 3
  umask 077
  # Compositor addresses can be recycled after a window exits. Retain the PID too;
  # a new launch must be explicitly repinned, even if its address happens to match.
  printf '%s\n%s\n' "$address" "$pid" >"$STATE"
  log "pin pid=$pid address=$address"
  echo "pinned $address (pid $pid)"
}

cmd_status() {
  local pin line why
  pin="$(pinned_address || true)"
  line="$(active_window)"
  echo "pinned: ${pin:-<none>}"
  echo "active: ${line//$'\t'/ }"
  if why="$(check 2>&1)"; then echo "OK: input would be sent"
  else echo "REFUSED: $why"; exit 3; fi
}

# A chord is optional ctrl+ / shift+ prefixes and one key: a letter, digit, or an allowed name.
ALLOWED_NAMES=" Return Escape Tab BackSpace Delete Up Down Left Right Home End Page_Up Page_Down space "
parse_chord() {   # prints "<mods...>|<key>" or fails
  local chord="$1" mods="" key rest="$1"
  while [[ "$rest" == *+* ]]; do
    local m="${rest%%+*}"
    case "${m,,}" in
      ctrl|shift) mods+="${m,,} " ;;
      *) echo "modifier '$m' is not allowed (only ctrl and shift: the compositor may bind others)" >&2; return 1 ;;
    esac
    rest="${rest#*+}"
  done
  key="$rest"
  if [[ "$key" =~ ^[A-Za-z0-9]$ || "$key" == "," || "$key" == "." ]]; then :
  elif [[ "$ALLOWED_NAMES" == *" $key "* ]]; then :
  else echo "key '$key' is not allowed" >&2; return 1; fi
  printf '%s|%s' "$mods" "$key"
}

send_chord() {
  local mods="${1%%|*}" key="${1##*|}" args=() m
  # wtype takes XKB keysym names, not punctuation characters, for -k.
  case "$key" in .) key=period ;; ,) key=comma ;; esac
  for m in $mods; do args+=(-M "$m"); done
  args+=(-k "$key")
  for m in $mods; do args+=(-m "$m"); done
  wtype "${args[@]}"
}

after_send() {
  if ! check 2>/dev/null; then
    log "FOCUS CHANGED after sending '$1'"
    die "focus changed after sending '$1'; stop and check where the last input went" 4
  fi
}

cmd_key() {
  (( $# >= 1 )) || usage
  (( $# <= MAX_KEYS )) || die "at most $MAX_KEYS keys per call" 2
  local parsed=() chord p why
  # Validate everything first, so a bad chord late in the list sends nothing at all.
  for chord in "$@"; do
    p="$(parse_chord "$chord")" || die "refused '$chord'" 3
    parsed+=("$p")
  done
  local i=0
  for chord in "$@"; do
    why="$(check 2>&1)" || die "refused: $why" 3
    log "key $chord"
    send_chord "${parsed[$i]}"
    after_send "$chord"
    i=$((i + 1))
    sleep 0.05
  done
}

cmd_text() {
  (( $# == 1 )) || usage
  local text="$1"
  (( ${#text} >= 1 && ${#text} <= MAX_TEXT )) || die "text must be 1..$MAX_TEXT characters" 3
  [[ "$text" != *[[:cntrl:]]* ]] || die "text must not contain control characters (use 'key Return' for Enter)" 3
  local why
  why="$(check 2>&1)" || die "refused: $why" 3
  log "text (${#text} chars)"
  wtype -- "$text"
  after_send "text"
}

need hyprctl; need jq
case "${1:-}" in
  pin)    shift; cmd_pin "$@" ;;
  status) shift; cmd_status ;;
  key)    shift; need wtype; cmd_key "$@" ;;
  text)   shift; need wtype; cmd_text "$@" ;;
  unpin)  rm -f "$STATE"; echo "unpinned" ;;
  *)      usage ;;
esac
