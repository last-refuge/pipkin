#!/usr/bin/env bash
# Pipkin installer.
#
#   curl -fsSL https://github.com/last-refuge/pipkin/releases/latest/download/install.sh | bash
#
# Linux x86_64: downloads the prebuilt release archive, checks it against the release's SHA256SUMS,
# and installs it into ~/.local with the archive's own installer. No root, no build, no Node, no Pi.
# It never compiles anything: if there is no prebuilt archive for this machine it stops and says so.
# Read it first if you like; it is short and does nothing you cannot see on the screen.
#
#   --version X.Y.Z   install that release instead of the latest
#   --prefix DIR      install into DIR instead of ~/.local
#   --dry-run         show what would happen and change nothing
#   --list            list the installed versions
#   --rollback        reactivate the previously installed version
#   --uninstall       remove Pipkin (app data in ~/.local/share/pipkin is kept)
#   --help            this text
set -Eeuo pipefail

main() {

REPO="last-refuge/pipkin"
ARCHIVE_GLOB='pipkin-native-*-x86_64.tar.zst'

# ---------------------------------------------------------------------------------------------
# Presentation
# ---------------------------------------------------------------------------------------------
if [[ -t 1 && -z "${NO_COLOR:-}" && "${TERM:-}" != dumb ]]; then
  TTY=1
  if [[ "${COLORTERM:-}" == *truecolor* || "${COLORTERM:-}" == *24bit* ]]; then
    ACCENT=$'\033[38;2;224;122;95m'; SAGE=$'\033[38;2;120;145;124m'
  else
    ACCENT=$'\033[33m'; SAGE=$'\033[32m'
  fi
  BOLD=$'\033[1m'; DIM=$'\033[2m'; RESET=$'\033[0m'; HIDE=$'\033[?25l'; SHOW=$'\033[?25h'
else
  TTY=0; ACCENT=''; SAGE=''; BOLD=''; DIM=''; RESET=''; HIDE=''; SHOW=''
fi

banner() {
  printf '\n  %s%spipkin%s  %s·  A native desktop coding agent.%s\n\n' "$ACCENT" "$BOLD" "$RESET" "$DIM" "$RESET"
}
ok()   { printf '  %s✔%s  %s\n' "$SAGE" "$RESET" "$*"; }
note() { printf '  %s·%s  %s%s%s\n' "$DIM" "$RESET" "$DIM" "$*" "$RESET"; }
warn() { printf '  %s!%s  %s\n' "$ACCENT" "$RESET" "$*"; }
die()  {
  printf '\n  %s✘%s  %s\n' "$ACCENT" "$RESET" "$*" >&2
  [[ -n "${LOG:-}" && -s "${LOG:-}" ]] && printf '     %sFull log: %s%s\n' "$DIM" "$LOG" "$RESET" >&2
  printf '\n' >&2
  KEEP_LOG=1
  exit 1
}

WORK=""; LOG=""; KEEP_LOG=0
cleanup() {
  printf '%s' "$SHOW"
  if [[ -n "$WORK" && -d "$WORK" ]]; then
    if (( KEEP_LOG )) && [[ -s "$LOG" ]]; then
      find "$WORK" -mindepth 1 -maxdepth 1 ! -name install.log -exec rm -rf {} +
    else
      rm -rf "$WORK"
    fi
  fi
  return 0
}
trap cleanup EXIT
trap 'die "Interrupted."' INT TERM

# Under `curl | bash` this script itself arrives on stdin, so no command may inherit it. Everything
# below reads nothing; the archive's installer asks no questions either.

# Run a command quietly with a spinner; its output goes to the log and is shown only if it fails.
step() {
  local msg=$1; shift
  if (( DRY )); then note "would: $msg"; return 0; fi
  if (( ! TTY )); then
    printf '  …  %s\n' "$msg"
    "$@" </dev/null >>"$LOG" 2>&1 || { tail -n 12 "$LOG" | sed 's/^/     /' >&2; die "$msg failed."; }
    ok "$msg"
    return 0
  fi
  "$@" </dev/null >>"$LOG" 2>&1 &
  local pid=$! i=0 frames='⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏'
  printf '%s' "$HIDE"
  while kill -0 "$pid" 2>/dev/null; do
    printf '\r  %s%s%s  %s' "$ACCENT" "${frames:i++%${#frames}:1}" "$RESET" "$msg"
    sleep 0.1
  done
  printf '%s\r\033[K' "$SHOW"
  if wait "$pid"; then ok "$msg"; else printf '\r\033[K'; tail -n 12 "$LOG" | sed 's/^/     /' >&2; die "$msg failed."; fi
}

usage() {
  cat <<'USAGE'
Pipkin installer.

  curl -fsSL https://github.com/last-refuge/pipkin/releases/latest/download/install.sh | bash

Linux x86_64: downloads the prebuilt release archive, checks it against the release's SHA256SUMS
and installs it into ~/.local with the archive's own installer. No root, no build, no Node, no Pi.

  --version X.Y.Z   install that release instead of the latest
  --prefix DIR      install into DIR instead of ~/.local
  --dry-run         show what would happen and change nothing
  --list            list the installed versions
  --rollback        reactivate the previously installed version
  --uninstall       remove Pipkin (app data in ~/.local/share/pipkin is kept)
  --help            this text

macOS: download the experimental DMG from https://github.com/last-refuge/pipkin/releases instead.
USAGE
}

# ---------------------------------------------------------------------------------------------
# Arguments
# ---------------------------------------------------------------------------------------------
VERSION=""; PREFIX=""; DRY=0; ACTION=install
while (( $# )); do
  case "$1" in
    --version) shift; [[ $# -gt 0 ]] || die "--version needs a version such as 1.1.0."; VERSION=${1#v} ;;
    --version=*) VERSION=${1#--version=}; VERSION=${VERSION#v} ;;
    --prefix) shift; [[ $# -gt 0 ]] || die "--prefix needs a directory."; PREFIX=$1 ;;
    --prefix=*) PREFIX=${1#--prefix=} ;;
    --dry-run) DRY=1 ;;
    --list) ACTION=list ;;
    --rollback) ACTION=rollback ;;
    --uninstall) ACTION=uninstall ;;
    --help|-h) usage; exit 0 ;;
    *) die "Unknown option $1 (try --help)." ;;
  esac
  shift
done
[[ -z "$VERSION" || "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+([-.][0-9A-Za-z.-]+)?$ ]] || die "--version must look like 1.1.0."
P=${PREFIX:-$HOME/.local}
[[ "$P" = /* && "$P" != / ]] || die "--prefix must be an absolute directory other than /."

WORK=$(mktemp -d "${TMPDIR:-/tmp}/pipkin-install.XXXXXX")
LOG="$WORK/install.log"

if [[ -n "$VERSION" ]]; then
  RELEASE_URL="https://github.com/$REPO/releases/download/v$VERSION"
else
  RELEASE_URL="https://github.com/$REPO/releases/latest/download"
fi

# ---------------------------------------------------------------------------------------------
# What kind of machine is this?
# ---------------------------------------------------------------------------------------------
banner
OS=$(uname -s); ARCH=$(uname -m)
IS_OMARCHY=0; DISTRO="this system"
if [[ "$OS" == Linux && -r /etc/os-release ]]; then
  # shellcheck disable=SC1091
  . /etc/os-release
  DISTRO=${PRETTY_NAME:-${NAME:-$DISTRO}}
fi
if [[ -d /usr/share/omarchy ]] || command -v omarchy >/dev/null 2>&1; then IS_OMARCHY=1; fi

case "$OS/$ARCH" in
  Linux/x86_64) ;;
  Darwin/*)
    die "This installer is for Linux. On a Mac, download the experimental DMG from https://github.com/$REPO/releases and drag Pipkin to Applications." ;;
  Linux/*)
    die "There is no prebuilt Pipkin for Linux $ARCH, and this installer never builds from source. Releases cover x86_64 only." ;;
  *)
    die "There is no prebuilt Pipkin for $OS. Releases cover Linux x86_64 and experimental macOS builds." ;;
esac
if (( IS_OMARCHY )); then ok "Omarchy detected"; else ok "$DISTRO detected"; fi
(( DRY )) && note "Dry run: nothing will be changed."

# ---------------------------------------------------------------------------------------------
# Installed-version actions: --list reads the prefix; --rollback and --uninstall use the archive's
# installer (a kept copy when there is one, otherwise the one in the downloaded release)
# ---------------------------------------------------------------------------------------------
INNER="$P/lib/pipkin/current/install.sh"

for t in curl tar sha256sum; do
  command -v "$t" >/dev/null 2>&1 || die "'$t' is needed. Install it with your package manager and run this again."
done
command -v zstd >/dev/null 2>&1 || die "'zstd' is needed to unpack the archive (on Arch: sudo pacman -S zstd). Install it and run this again."

if [[ "$ACTION" == list ]]; then
  VERSIONS="$P/lib/pipkin/versions"; CUR=""
  [[ -L "$P/lib/pipkin/current" ]] && CUR=$(basename "$(readlink "$P/lib/pipkin/current")")
  if [[ -d "$VERSIONS" ]] && ls -1 "$VERSIONS" 2>/dev/null | grep -q .; then
    printf '\n'
    for v in $(ls -1t "$VERSIONS"); do
      if [[ "$v" == "$CUR" ]]; then printf '  %s*%s %s  %s(current)%s\n' "$SAGE" "$RESET" "$v" "$DIM" "$RESET"; else printf '    %s\n' "$v"; fi
    done
    printf '\n'
  else
    note "No versioned Pipkin installation found in $P."
  fi
  exit 0
fi
if [[ "$ACTION" != install ]]; then
  if [[ -x "$INNER" ]]; then
    if (( DRY )); then note "would run the installed copy: install.sh --$ACTION --prefix $P"; exit 0; fi
    exec "$INNER" --prefix "$P" "--$ACTION" </dev/null
  fi
  note "Fetching the release archive to run its installer with --$ACTION."
fi

# ---------------------------------------------------------------------------------------------
# Download and verify
# ---------------------------------------------------------------------------------------------
if (( DRY )); then
  if curl -fsIL --retry 1 -o /dev/null "$RELEASE_URL/SHA256SUMS" 2>/dev/null; then
    note "would download SHA256SUMS and the x86_64 archive from $RELEASE_URL"
    note "would verify the archive's checksum, unpack it, and run its install.sh --prefix $P"
  else
    die "No release found at $RELEASE_URL."
  fi
  exit 0
fi

step "Fetching the release checksums" curl -fsSL --retry 2 -o "$WORK/SHA256SUMS" "$RELEASE_URL/SHA256SUMS"
ARCHIVE=$(awk '{print $2}' "$WORK/SHA256SUMS" | grep -E '^pipkin-native-[0-9][^ /]*-x86_64\.tar\.zst$' || true)
[[ -n "$ARCHIVE" ]] || die "The release at $RELEASE_URL has no Linux x86_64 archive."
[[ $(printf '%s\n' "$ARCHIVE" | wc -l) -eq 1 ]] || die "The release lists more than one Linux archive; refusing to guess."
REL_VERSION=${ARCHIVE#pipkin-native-}; REL_VERSION=${REL_VERSION%-x86_64.tar.zst}
ok "Release $REL_VERSION"

step "Downloading $ARCHIVE" curl -fsSL --retry 2 -o "$WORK/$ARCHIVE" "$RELEASE_URL/$ARCHIVE"
verify() { cd "$WORK" && sha256sum --check --ignore-missing --strict SHA256SUMS && grep -q "$ARCHIVE" SHA256SUMS; }
step "Verifying the checksum" verify
unpack() { mkdir -p "$WORK/pkg" && tar --zstd -C "$WORK/pkg" -xf "$WORK/$ARCHIVE" && test -x "$WORK/pkg/install.sh"; }
step "Unpacking the archive" unpack

# ---------------------------------------------------------------------------------------------
# Install with the archive's own installer, which shows its own progress
# ---------------------------------------------------------------------------------------------
if [[ "$ACTION" != install ]]; then
  exec "$WORK/pkg/install.sh" --prefix "$P" "--$ACTION" </dev/null
fi

compgen -G "/usr/share/vulkan/icd.d/*.json" >/dev/null ||
  warn "No Vulkan driver found. Pipkin draws with Vulkan; install your GPU's driver (for example vulkan-intel, vulkan-radeon or nvidia-utils)."
printf '\n'
"$WORK/pkg/install.sh" --prefix "$P" </dev/null || die "The archive's installer failed. Nothing was activated."
printf '\n'

if [[ -x "$P/bin/pipkin" ]]; then
  ok "Installed $("$P/bin/pipkin" --version 2>/dev/null || echo "Pipkin $REL_VERSION") into $P"
fi
case ":$PATH:" in *":$P/bin:"*) ;; *) warn "$P/bin is not on your PATH; add it to run 'pipkin' by name." ;; esac
if (( IS_OMARCHY )); then
  note "Open it with Super+Space and type Pipkin, or run: $P/bin/pipkin"
else
  note "Open Pipkin from your app menu, or run: $P/bin/pipkin"
fi
note "Later: this script with --rollback, --uninstall or --list; or re-run it to upgrade."
printf '\n'

}

main "$@"
