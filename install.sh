#!/usr/bin/env sh
set -eu

repo="treeleaves30760/all-code"
if [ -n "${ALC_INSTALL_DIR:-}" ]; then
  install_dir="$ALC_INSTALL_DIR"
  default_install="no"
else
  install_dir="$HOME/.local/bin"
  default_install="yes"
fi
version="${ALC_VERSION:-latest}"

die() {
  printf 'alc installer: %s\n' "$*" >&2
  exit 1
}

command_exists() {
  command -v "$1" >/dev/null 2>&1
}

download() {
  source_url="$1"
  destination="$2"
  if command_exists curl; then
    curl -fsSL --retry 3 "$source_url" -o "$destination"
  elif command_exists wget; then
    wget -q --tries=3 "$source_url" -O "$destination"
  else
    die "curl or wget is required"
  fi
}

case "$(uname -s)" in
  Linux) os="linux" ;;
  Darwin) os="darwin" ;;
  *) die "unsupported operating system: $(uname -s)" ;;
esac

case "$(uname -m)" in
  x86_64 | amd64) arch="x86_64" ;;
  arm64 | aarch64) arch="aarch64" ;;
  *) die "unsupported CPU architecture: $(uname -m)" ;;
esac

asset="alc-${os}-${arch}.tar.gz"
if [ "$version" = "latest" ]; then
  release_url="https://github.com/${repo}/releases/latest/download"
else
  case "$version" in
    v*) tag="$version" ;;
    *) tag="v$version" ;;
  esac
  release_url="https://github.com/${repo}/releases/download/${tag}"
fi

tmp_dir="$(mktemp -d 2>/dev/null || mktemp -d -t alc-install)"
cleanup() {
  if [ -n "${tmp_dir:-}" ] && [ -d "$tmp_dir" ]; then
    rm -rf -- "$tmp_dir"
  fi
}
trap cleanup EXIT HUP INT TERM

archive="$tmp_dir/$asset"
checksums="$tmp_dir/checksums.txt"
printf 'Downloading %s...\n' "$asset"
download "$release_url/$asset" "$archive"
download "$release_url/checksums.txt" "$checksums"

expected="$(awk -v name="$asset" '$2 == name || $2 == "*" name { print $1; exit }' "$checksums")"
[ -n "$expected" ] || die "no checksum was published for $asset"

if command_exists sha256sum; then
  actual="$(sha256sum "$archive" | awk '{print $1}')"
elif command_exists shasum; then
  actual="$(shasum -a 256 "$archive" | awk '{print $1}')"
else
  die "sha256sum or shasum is required to verify the download"
fi
[ "$actual" = "$expected" ] || die "checksum mismatch for $asset"

extract_dir="$tmp_dir/extract"
mkdir -p "$extract_dir"
tar -xzf "$archive" -C "$extract_dir"
[ -f "$extract_dir/alc" ] || die "release archive does not contain alc"
[ -f "$extract_dir/claude-codex" ] || die "release archive does not contain claude-codex"

mkdir -p "$install_dir"
install_dir="$(cd "$install_dir" && pwd -P)"
if command_exists install; then
  install -m 0755 "$extract_dir/alc" "$install_dir/alc"
  install -m 0755 "$extract_dir/claude-codex" "$install_dir/claude-codex"
else
  cp "$extract_dir/alc" "$install_dir/alc"
  cp "$extract_dir/claude-codex" "$install_dir/claude-codex"
  chmod 0755 "$install_dir/alc" "$install_dir/claude-codex"
fi

path_status="present"
profile=""
case ":$PATH:" in
  *":$install_dir:"*) ;;
  *)
    path_status="missing"
    case "${SHELL:-}" in
      */zsh) profile="$HOME/.zshrc" ;;
      */bash) profile="$HOME/.bashrc" ;;
      *) profile="$HOME/.profile" ;;
    esac
    if [ "${ALC_NO_PATH_UPDATE:-0}" != "1" ] && [ "$default_install" = "yes" ]; then
      marker='# Added by the alc installer'
      if grep -F "$marker" "$profile" >/dev/null 2>&1; then
        path_status="profile"
      elif {
          printf '\n%s\n' "$marker"
          printf 'export PATH="$HOME/.local/bin:$PATH"\n'
        } >> "$profile"; then
        path_status="profile"
      else
        path_status="failed"
      fi
    fi
    ;;
esac

printf '\nInstalled alc to %s\n' "$install_dir/alc"
case "$path_status" in
  present)
    printf 'alc is already available on PATH. Run: alc config\n'
    ;;
  profile)
    printf 'Added ~/.local/bin to PATH in %s.\n' "$profile"
    printf 'Restart your terminal (or run: source "%s"), then run: alc config\n' "$profile"
    ;;
  failed)
    printf 'Could not update %s automatically.\n' "$profile" >&2
    printf 'Add this line manually, then restart your terminal:\n' >&2
    printf '  export PATH="%s:$PATH"\n' "$install_dir" >&2
    ;;
  missing)
    printf 'alc is installed, but %s is not on PATH.\n' "$install_dir" >&2
    printf 'Add this line to %s, then restart your terminal:\n' "$profile" >&2
    printf '  export PATH="%s:$PATH"\n' "$install_dir" >&2
    ;;
esac
