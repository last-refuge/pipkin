#!/usr/bin/env bash
# Install the identified Linux archive into a versioned, user-owned prefix. No sudo, network,
# credential handling or changes to the user's Node installation. The GUI owns account setup.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
prefix=${PREFIX:-$HOME/.local}
keep=2
action=install
while (($#)); do
  case "$1" in
    --prefix) (($# >= 2)) || { echo '--prefix needs a directory' >&2; exit 2; }; prefix=$2; shift 2 ;;
    --keep) (($# >= 2)) || { echo '--keep needs a number' >&2; exit 2; }; keep=$2; shift 2 ;;
    --list) action=list; shift ;;
    --rollback) action=rollback; shift ;;
    --uninstall) action=uninstall; shift ;;
    -h|--help)
      printf '%s\n' 'Usage: ./install.sh [--prefix DIR] [--keep N] [--list | --rollback | --uninstall]' \
        'Run from the extracted x86_64 Linux archive. Default prefix: ~/.local.' \
        'The built-in Rust engine needs no external runtime; provider setup happens in the app.'
      exit 0 ;;
    *) echo "unknown option: $1" >&2; exit 2 ;;
  esac
done
[[ "$prefix" = /* && "$prefix" != / ]] || { echo 'prefix must be an absolute directory other than /' >&2; exit 2; }
[[ "$keep" =~ ^[1-9][0-9]*$ ]] || { echo '--keep must be a positive integer' >&2; exit 2; }
base=$prefix/lib/pipkin
versions=$base/versions
current=$base/current
incoming=''
scratch=''
candidate=''
activated=0
created=0
cleanup() {
  if [[ -n "$incoming" ]]; then rm -rf -- "$incoming"; fi
  if [[ -n "$candidate" && "$created" = 1 && "$activated" = 0 ]]; then rm -rf -- "$candidate"; fi
  if [[ -n "$scratch" ]]; then rm -rf -- "$scratch"; fi
}
trap cleanup EXIT

if [[ -t 1 && -z "${NO_COLOR:-}" && "${TERM:-}" != dumb ]]; then
  blue=$'\033[36m'; green=$'\033[32m'; yellow=$'\033[33m'; red=$'\033[31m'; reset=$'\033[0m'
else
  blue=''; green=''; yellow=''; red=''; reset=''
fi
step=0
progress() {
  local done=$1 title=$2 width=18 fill empty
  step=$((step + 1))
  fill=$((step * width / 6)); empty=$((width - fill))
  printf '%b[%s]%b [%s%s] %3d%%  %s\n' \
    "$green" "$done" "$reset" "$(printf '%*s' "$fill" '' | tr ' ' '#')" \
    "$(printf '%*s' "$empty" '' | tr ' ' '-')" "$((step * 100 / 6))" "$title"
}
die() { printf '%b[FAIL]%b %s\n' "$red" "$reset" "$*" >&2; exit 1; }
note() { printf '%b[NOTE]%b %s\n' "$yellow" "$reset" "$*"; }
app_version() { "$1/usr/bin/pipkin" --version 2>/dev/null | awk '{print $2}'; }
installed() { ls -1t "$versions" 2>/dev/null || true; }
current_name() { [[ -L "$current" ]] && basename "$(readlink "$current")" || true; }
guard_links() {
  local binary=$prefix/bin/pipkin desktop=$prefix/share/applications/pipkin.desktop
  if [[ -e "$binary" || -L "$binary" ]]; then
    [[ -L "$binary" && $(readlink "$binary") = "$current/usr/bin/pipkin" ]] ||
      die "Refusing to replace another program at $binary. Choose a different --prefix."
  elif [[ -e "$desktop" ]]; then
    die "Refusing to replace an existing desktop launcher at $desktop. Choose a different --prefix."
  fi
}
link_all() {
  local name=$1 root=$versions/$1 pending=$base/.current-$$ size icon
  mkdir -p "$prefix/bin" "$prefix/share/applications"
  ln -s "$root" "$pending"
  mv -Tf "$pending" "$current" # one switch: the old version remains until now
  activated=1
  ln -sfn "$current/usr/bin/pipkin" "$prefix/bin/pipkin"
  awk -v binary="$prefix/bin/pipkin" '
    /^Exec=pipkin( |$)/ {
      rest = substr($0, length("Exec=pipkin") + 1)
      gsub(/\\/, "\\\\", binary)
      gsub(/"/, "\\\"", binary)
      print "Exec=\"" binary "\"" rest; next
    }
    { print }
  ' "$root/usr/share/applications/pipkin.desktop" > "$prefix/share/applications/pipkin.desktop"
  for size in 128 256 512; do
    icon="$root/usr/share/icons/hicolor/${size}x${size}/apps/pipkin.png"
    [[ ! -f "$icon" ]] || install -Dm644 "$icon" "$prefix/share/icons/hicolor/${size}x${size}/apps/pipkin.png"
  done
  command -v update-desktop-database >/dev/null && update-desktop-database "$prefix/share/applications" 2>/dev/null || true
  command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q "$prefix/share/icons/hicolor" 2>/dev/null || true
}

probe_version() {
  local root=$1 native=$2
  scratch=$(mktemp -d /tmp/pipkin-install-XXXXXX)
  mkdir -p "$scratch/home"
  local probe_args=(--diagnose --probe) legacy_env=()
  [[ "$native" = 1 ]] || legacy_env+=(PI_OFFLINE=1)
  [[ "$native" = 0 ]] || probe_args+=(--engine native)
  if ! env -i HOME="$scratch/home" XDG_DATA_HOME="$scratch/home/.local/share" \
    TMPDIR="$scratch" PATH=/usr/bin:/bin "${legacy_env[@]}" \
    "$root/usr/bin/pipkin" "${probe_args[@]}" >"$scratch/probe.log" 2>&1; then
    die 'The installed engine failed its offline handshake. The current version is unchanged; inspect local libraries and try again.'
  fi
  grep -q 'engine probe: OK' "$scratch/probe.log" || die 'The installed engine did not answer its offline handshake. The current version is unchanged.'
}

verify_rollback_target() {
  local root=$1 expected info=$1/usr/share/doc/pipkin/build-info.json native=0
  [[ ! -L "$root" && -x "$root/usr/bin/pipkin" && -f "$info" &&
     -f "$root/usr/share/applications/pipkin.desktop" ]] || die 'The rollback package is incomplete. The current version is unchanged.'
  expected=$(sed -n 's/.*"binarySha256": "\([0-9a-f]\{64\}\)".*/\1/p' "$info" | head -1)
  [[ ${#expected} = 64 && $(sha256sum "$root/usr/bin/pipkin" | awk '{print $1}') = "$expected" ]] ||
    die 'Rollback executable checksum mismatch. The current version is unchanged.'
  if [[ -f "$root/usr/lib/pipkin/native-engine.json" ]]; then
    native=1
    [[ ! -e "$root/usr/lib/pipkin/engine" && ! -e "$root/usr/lib/pipkin/runtime" ]] ||
      die 'The native rollback package also carries a legacy engine or Node runtime. The current version is unchanged.'
  else
    # Explicit rollback may reactivate an intact, previously installed historical Pi build.
    # New installs never ship or provision this retired runtime.
    local runtime=$root/usr/lib/pipkin/runtime engine=$root/usr/lib/pipkin/engine
    [[ -x "$runtime/bin/node" && -f "$runtime/runtime.json" && -f "$engine/engine.json" &&
       -f "$engine/pi-test.sh" && -d "$engine/node_modules" ]] || die 'The paired rollback engine is incomplete. The current version is unchanged.'
    expected=$(sed -n 's/.*"binarySha256": "\([0-9a-f]\{64\}\)".*/\1/p' "$runtime/runtime.json" | head -1)
    [[ ${#expected} = 64 && $(sha256sum "$runtime/bin/node" | awk '{print $1}') = "$expected" ]] ||
      die 'Rollback Node checksum mismatch. The current version is unchanged.'
  fi
  probe_version "$root" "$native"
}

case "$action" in
  install)
    printf '\n%bPIPKIN  /  INSTALL%b\n\n' "$blue" "$reset"
    [[ $(uname -s) = Linux && $(uname -m) = x86_64 ]] || die 'This archive needs x86_64 Linux; no files were installed.'
    for tool in sha256sum sed awk tr mktemp cp mv install env grep uname; do
      command -v "$tool" >/dev/null || die "Missing basic system tool: $tool. Install coreutils and retry."
    done
    native=1
    [[ -f "$here/usr/lib/pipkin/native-engine.json" && -x "$here/usr/bin/pipkin" ]] || die 'Run this from a complete native Pipkin archive. Installing new legacy runtime packages is retired.'
    [[ ! -e "$here/usr/lib/pipkin/engine" && ! -e "$here/usr/lib/pipkin/runtime" ]] || die 'A native-engine archive must not also carry a legacy engine or Node runtime.'
    [[ ! -L "$versions" ]] || die 'Refusing a symlinked installation versions directory.'
    guard_links
    command -v git >/dev/null || die 'Git is required for project tools. Install it with your distribution package manager (Arch: sudo pacman -S git), then retry.'
    if command -v ldd >/dev/null; then
      missing=$(ldd "$here/usr/bin/pipkin" 2>&1 | awk '/ => not found/ {print $1}' | tr '\n' ' ') || true
      [[ -z "$missing" ]] || die "Missing native libraries: $missing. Install their distribution packages and retry."
    fi
    progress OK 'Linux, Git and native libraries'

    progress OK 'Built-in native engine: no Node runtime needed; no system changes'

    info="$here/usr/share/doc/pipkin/build-info.json"
    [[ -f "$info" ]] || die 'Native app build identity is missing.'
    expected=$(sed -n 's/.*"binarySha256": "\([0-9a-f]\{64\}\)".*/\1/p' "$info" | head -1)
    [[ ${#expected} = 64 && $(sha256sum "$here/usr/bin/pipkin" | awk '{print $1}') = "$expected" ]] || die 'Pipkin executable checksum mismatch; do not run this archive.'
    "$here/usr/bin/pipkin" --version >/dev/null || die 'Pipkin cannot start here. Check your Linux library/GLIBC compatibility.'
    "$here/usr/bin/pipkin" --version | grep -q 'engine native' || die 'This Pipkin does not identify the native engine as its default.'
    name="$(app_version "$here")-native-${expected:0:12}"
    [[ "$name" != -* && "$name" != .* && "$name" != *[!a-zA-Z0-9._-]* ]] || die 'Invalid app or engine version.'
    progress OK 'Pipkin and built-in native engine verified'

    mkdir -p "$versions"
    candidate=$versions/$name
    [[ ! -L "$candidate" ]] || die 'Refusing a symlinked installed version.'
    if [[ ! -d "$candidate" ]]; then
      incoming=$(mktemp -d "$versions/.incoming-XXXXXX")
      cp -a "$here/usr" "$incoming/usr" || die 'Could not copy the package; the current version is unchanged.'
      mv "$incoming" "$candidate"
      incoming=''
      created=1
    fi
    progress OK "Copied version to $prefix"

    # A throwaway offline profile; this neither touches Pi credentials nor makes model requests.
    probe_version "$candidate" "$native"
    progress OK 'Installed app and engine passed an offline handshake'

    link_all "$name"
    count=0
    for v in $(installed); do
      count=$((count + 1))
      if ((count > keep)) && [[ "$v" != "$(current_name)" ]]; then rm -rf -- "$versions/$v"; fi
    done
    progress OK 'Desktop shortcut ready; existing app data preserved'
    if [[ -z "${WAYLAND_DISPLAY:-}" && -z "${DISPLAY:-}" ]]; then
      note 'No graphical display is active in this shell; open Pipkin from a desktop session.'
    fi
    printf '\nOpen Pipkin from your app menu or run %s/bin/pipkin.\n' "$prefix"
    printf '%s\n' 'Sign in, choose a project and select a model in the Pipkin GUI.' \
      'No account credentials were requested or sent during installation.'
    ;;
  list)
    cur=$(current_name)
    for v in $(installed); do
      if [[ "$v" = "$cur" ]]; then echo "* $v (current)"; else echo "  $v"; fi
    done
    ;;
  rollback)
    guard_links
    cur=$(current_name); prev=''
    for v in $(installed); do [[ "$v" = "$cur" ]] || { prev=$v; break; }; done
    [[ -n "$prev" ]] || die 'No previous installed version to roll back to.'
    verify_rollback_target "$versions/$prev"
    link_all "$prev"
    touch "$versions/$prev"
    echo "Rolled back to $prev (was $cur). App data was not rolled back."
    ;;
  uninstall)
    [[ -L "$current" ]] || die 'No versioned Pipkin installation was found; nothing was removed.'
    guard_links
    rm -f "$prefix/bin/pipkin" "$prefix/share/applications/pipkin.desktop"
    for size in 128 256 512; do rm -f "$prefix/share/icons/hicolor/${size}x${size}/apps/pipkin.png"; done
    rm -rf -- "$base"
    echo "Removed Pipkin from $prefix. Your app data remains in ~/.local/share/pipkin."
    ;;
esac
