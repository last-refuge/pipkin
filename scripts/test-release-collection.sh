#!/usr/bin/env bash
# Test release collection without building/publishing/signing a real release.
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd)
root=$(mktemp -d)
trap 'rm -rf "$root"' EXIT
mkdir -p "$root/scripts" "$root/bin"
cp "$here/scripts/release.sh" "$root/scripts/"
cat > "$root/bin/cargo" <<'SH'
#!/usr/bin/env bash
printf 'pipkin-app@0.0.1\n'
SH
cat > "$root/scripts/package.sh" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
mkdir -p dist/native
printf 'current\n' > "dist/native/pipkin-native-0.0.1-$(uname -m).tar.zst"
printf 'obsolete\n' > dist/native/pipkin-native-0.0.0-obsolete.tar.zst
SH
chmod +x "$root/bin/cargo" "$root/scripts/package.sh"
if PATH="$root/bin:$PATH"  PIPKIN_SIGN_KEY= "$root/scripts/release.sh" > "$root/output" 2>&1; then
  echo 'unidentified app release accepted' >&2; exit 1
fi
grep -q 'identified app sources' "$root/output"
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1
export GIT_AUTHOR_NAME=test GIT_AUTHOR_EMAIL=test@example.invalid GIT_COMMITTER_NAME=test GIT_COMMITTER_EMAIL=test@example.invalid
printf 'dist/\noutput\n' > "$root/.gitignore"
git -C "$root" init -q
git -C "$root" add .
git -C "$root" commit -qm fixture
printf 'dirty\n' > "$root/untracked"
if PATH="$root/bin:$PATH"  PIPKIN_SIGN_KEY= "$root/scripts/release.sh" > "$root/output" 2>&1; then
  echo 'dirty app release accepted' >&2; exit 1
fi
grep -q 'clean app checkout' "$root/output"
rm "$root/untracked"
PATH="$root/bin:$PATH"  PIPKIN_SIGN_KEY= "$root/scripts/release.sh"
[ -f "$root/dist/release/pipkin-native-0.0.1-$(uname -m).tar.zst" ]
[ ! -e "$root/dist/release/pipkin-native-0.0.0-obsolete.tar.zst" ]
(cd "$root/dist/release" && sha256sum -c SHA256SUMS)
echo 'identified clean source enforcement and exact native artifact collection verified'
