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

tmux_ready() {
  tmux_reason='tmux is not on PATH'
  command_exists tmux || return 1
  if ! tmux_version=$(tmux -V </dev/null 2>/dev/null); then
    tmux_reason='tmux -V failed for the first tmux on PATH'
    return 1
  fi
  # Like alc, read major.minor from the first line, ignoring patch suffixes.
  if printf '%s\n' "$tmux_version" | awk '
    NR == 1 {
      sub(/^[^0-9]*/, "")
      if (!match($0, /^[0-9]+\.[0-9]+/)) exit 1
      split(substr($0, 1, RLENGTH), v, ".")
      exit !(v[1] <= 4294967295 && v[2] <= 4294967295 &&
             (v[1] > 3 || (v[1] == 3 && v[2] >= 2)))
    }'; then
    return 0
  fi
  tmux_reason='the first tmux on PATH is older than 3.2 or its version cannot be parsed'
  return 1
}

tmux_warning() {
  printf 'alc installer: warning: %s. alc is installed; only --tmux needs tmux 3.2+.\n' "$1" >&2
  printf 'Install or upgrade manually, then check PATH and tmux -V:\n  %s\n' "$tmux_manual" >&2
}

tmux_package() {
  # Never let a package command read a piped install script or prompt for sudo.
  if [ "$tmux_use_sudo" = yes ]; then
    sudo -n "$@" </dev/null
  else
    "$@" </dev/null
  fi
}

install_tmux() {
  if [ "${ALC_NO_TMUX_INSTALL:-0}" = 1 ]; then
    printf 'Skipping tmux dependency setup (ALC_NO_TMUX_INSTALL=1).\n'
    return
  fi
  if tmux_ready; then
    printf 'tmux is ready for --tmux (3.2 or newer).\n'
    return
  fi

  tmux_use_sudo=no
  if [ "$os" = darwin ]; then
    tmux_manual='brew install tmux'
    if ! command_exists brew; then
      tmux_warning "$tmux_reason; Homebrew is not installed (not installed automatically)"
      return
    fi
    if brew list --versions tmux </dev/null >/dev/null 2>&1; then
      tmux_manual='brew upgrade tmux'
      set -- brew upgrade tmux
    else
      set -- brew install tmux
    fi
  else
    tmux_manager=''
    for candidate in apt-get dnf yum pacman zypper apk; do
      if command_exists "$candidate"; then
        tmux_manager=$candidate
        break
      fi
    done
    case "$tmux_manager" in
      apt-get) set -- apt-get install -y tmux; tmux_manual='sudo apt-get update && sudo apt-get install -y tmux' ;;
      dnf) set -- dnf install -y tmux; tmux_manual='sudo dnf install -y tmux' ;;
      yum) set -- yum install -y tmux; tmux_manual='sudo yum install -y tmux' ;;
      pacman) set -- pacman -S --needed --noconfirm tmux; tmux_manual='sudo pacman -S --needed tmux' ;;
      zypper) set -- zypper --non-interactive install tmux; tmux_manual='sudo zypper install tmux' ;;
      apk) set -- apk add --upgrade tmux; tmux_manual='sudo apk add --upgrade tmux' ;;
      *)
        tmux_manual='Install tmux 3.2+ with your package manager (e.g. sudo apt-get update && sudo apt-get install -y tmux).'
        tmux_warning "$tmux_reason; no supported package manager was found"
        return
        ;;
    esac
    if [ "$(id -u)" != 0 ]; then
      if ! command_exists sudo; then
        tmux_warning "$tmux_reason; root privileges or sudo are required"
        return
      fi
      if sudo -n -v </dev/null 2>/dev/null; then
        tmux_use_sudo=yes
      elif { [ -t 1 ] || [ -t 2 ]; } && ( : </dev/tty ) 2>/dev/null; then
        # A controlling terminal is separate from the curl | sh script input.
        if sudo -v </dev/tty; then
          tmux_use_sudo=yes
        else
          tmux_warning "$tmux_reason; sudo authentication failed"
          return
        fi
      else
        tmux_warning "$tmux_reason; sudo credentials are unavailable without an interactive terminal"
        return
      fi
    fi
    if [ "$tmux_manager" = apt-get ]; then
      if ! tmux_package apt-get update; then
        tmux_warning 'tmux package index update failed'
        return
      fi
    fi
  fi

  printf 'Installing or upgrading optional tmux using %s...\n' "$1"
  if ! tmux_package "$@"; then
    tmux_warning 'tmux package installation failed'
  fi
  # Forget cached executable locations and verify the version alc will see,
  # including when the manager installed tmux before reporting a later failure.
  hash -r 2>/dev/null || :
  if tmux_ready; then
    printf 'tmux is ready for --tmux (3.2 or newer).\n'
  else
    tmux_warning "$tmux_reason after package installation; an older PATH entry may be hiding it"
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

mkdir -p "$install_dir"
install_dir="$(cd "$install_dir" && pwd -P)"
if command_exists install; then
  install -m 0755 "$extract_dir/alc" "$install_dir/alc"
else
  cp "$extract_dir/alc" "$install_dir/alc"
  chmod 0755 "$install_dir/alc"
fi

# The Codex bridge is built into alc from 1.4.0 on. An older install left a
# separate claude-codex here; removing it keeps a stale copy from answering
# for anyone who still calls it directly.
rm -f "$install_dir/claude-codex"

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
    printf 'alc is already available on PATH.\n'
    printf 'Next: codex login, then: alc --codex claude\n'
    printf 'Another provider instead: alc config\n'
    ;;
  profile)
    printf 'Added ~/.local/bin to PATH in %s.\n' "$profile"
    printf 'Restart your terminal (or run: source "%s").\n' "$profile"
    printf 'Then: codex login, then: alc --codex claude\n'
    printf 'Another provider instead: alc config\n'
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

install_tmux
