#!/usr/bin/env sh
# Offline integration tests: only whitelisted utilities and fixture commands can
# appear on the installer's PATH. No network, real package manager, or user HOME.
set -eu

repo_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
shell=$(command -v sh)
python=$(command -v python3 || :)
test_dir=$(mktemp -d)
trap 'rm -rf "$test_dir"' EXIT HUP INT TERM
tr -d '\r' < "$repo_dir/install.sh" > "$test_dir/install.sh"
# More than a shell input buffer: a dependency command reading script stdin
# would eat either these comments or the final sentinel.
awk 'BEGIN { for (i = 0; i < 2000; i++) print "# piped installer stdin must not be consumed" }' >> "$test_dir/install.sh"
printf '\nprintf "script completed\\n"\n' >> "$test_dir/install.sh"

# A real controlling tty, without making the installer's script stdin a tty.
# This exercises both allowed prompt channels and the fully redirected case.
cat > "$test_dir/terminal.py" <<'PY'
import errno
import fcntl
import os
import pty
import sys
import termios

master, slave = pty.openpty()
pid = os.fork()
if pid == 0:
    os.close(master)
    os.setsid()
    fcntl.ioctl(slave, termios.TIOCSCTTY, 0)
    if sys.argv[2] == "hidden":
        # Keep the controlling tty open even though neither output uses it.
        os.set_inheritable(slave, True)
    else:
        os.dup2(slave, 1 if sys.argv[2] == "stdout" else 2)
        os.close(slave)
    os.execv(sys.argv[1], [sys.argv[1]])
os.close(slave)
while True:
    try:
        chunk = os.read(master, 4096)
    except OSError as error:
        if error.errno == errno.EIO:
            break
        raise
    if not chunk:
        break
    sys.stdout.buffer.write(chunk.replace(b"\r\n", b"\n"))
    sys.stdout.buffer.flush()
os.close(master)
_, status = os.waitpid(pid, 0)
sys.exit(os.WEXITSTATUS(status) if os.WIFEXITED(status) else 1)
PY

mkdir -p "$test_dir/utilities" "$test_dir/archive"
for utility in awk cat chmod cp grep gzip install mkdir mktemp rm tar; do
  utility_path=$(command -v "$utility")
  printf '#!%s\nexec "%s" "$@"\n' "$shell" "$utility_path" > "$test_dir/utilities/$utility"
  chmod +x "$test_dir/utilities/$utility"
done
if command -v sha256sum >/dev/null 2>&1; then
  hash_command=sha256sum
else
  hash_command=shasum
fi
hash_path=$(command -v "$hash_command")
printf '#!%s\nexec "%s" "$@"\n' "$shell" "$hash_path" > "$test_dir/utilities/$hash_command"
chmod +x "$test_dir/utilities/$hash_command"
printf '#!%s\nprintf "alc fixture\\n"\n' "$shell" > "$test_dir/archive/alc"
chmod +x "$test_dir/archive/alc"
tar -czf "$test_dir/release.tar.gz" -C "$test_dir/archive" alc
if [ "$hash_command" = sha256sum ]; then
  digest=$(sha256sum "$test_dir/release.tar.gz" | awk '{print $1}')
else
  digest=$(shasum -a 256 "$test_dir/release.tar.gz" | awk '{print $1}')
fi

passed=0
fail() { printf 'FAIL: %s: %s\n' "$name" "$*" >&2; cat "$fixture/output" >&2; exit 1; }
assert_output() { grep -E "$1" "$fixture/output" >/dev/null || fail "missing output: $1"; }
assert_no_output() { if grep -E "$1" "$fixture/output" >/dev/null; then fail "unexpected output: $1"; fi; }
assert_calls() {
  actual=$(cat "$fixture/calls")
  [ "$actual" = "$1" ] || fail "expected calls [$1], got [$actual]"
}
pass() { passed=$((passed + 1)); printf 'PASS: %s\n' "$name"; }

new_case() {
  name=$1
  fixture="$test_dir/$name"
  mkdir -p "$fixture/bin" "$fixture/home" "$fixture/tmp"
  cp "$test_dir/utilities/"* "$fixture/bin/"
  : > "$fixture/calls"
  os=Linux uid=0 manager_exit=0 after_version='tmux 3.6a'
  no_tmux=0 no_path=1 cached_sudo=1 bad_checksum=0 brew_installed=0
  terminal=none sudo_auth=1
  printf '#!%s\n' "$shell" > "$fixture/bin/uname"
  cat >> "$fixture/bin/uname" <<'STUB'
case "$1" in -s) printf '%s\n' "$TEST_OS" ;; -m) printf 'x86_64\n' ;; *) exit 90 ;; esac
STUB
  printf '#!%s\n' "$shell" > "$fixture/bin/id"
  cat >> "$fixture/bin/id" <<'STUB'
[ "$*" = '-u' ] || exit 90
printf '%s\n' "$TEST_UID"
STUB
  printf '#!%s\n' "$shell" > "$fixture/bin/curl"
  cat >> "$fixture/bin/curl" <<'STUB'
[ "$1 $2 $3" = '-fsSL --retry 3' ] && [ "$5" = '-o' ] || exit 90
case "$4" in
  */alc-*.tar.gz) cp "$TEST_ARCHIVE" "$6" ;;
  */checksums.txt)
    if [ "$TEST_BAD_CHECKSUM" = 1 ]; then digest=bad; else digest=$TEST_DIGEST; fi
    printf '%s  alc-%s-x86_64.tar.gz\n' "$digest" "$(printf '%s' "$TEST_OS" | awk '{print tolower($0)}')" > "$6"
    ;;
  *) exit 90 ;;
esac
STUB
  printf '#!%s\n' "$shell" > "$fixture/tmux-stub"
  cat >> "$fixture/tmux-stub" <<'STUB'
[ "$*" = '-V' ] || exit 90
if IFS= read -r line; then printf 'tmux consumed stdin\n' >&2; exit 91; fi
cat "$TEST_FIXTURE/version"
exit "$(cat "$TEST_FIXTURE/tmux-exit")"
STUB
  printf '0\n' > "$fixture/tmux-exit"
  chmod +x "$fixture/bin/"* "$fixture/tmux-stub"
}

with_tmux() {
  printf '%s\n' "$1" > "$fixture/version"
  cp "$fixture/tmux-stub" "$fixture/bin/tmux"
}

with_manager() {
  package_manager=$1
  printf '#!%s\n' "$shell" > "$fixture/bin/$1"
  cat >> "$fixture/bin/$1" <<'STUB'
manager=${0##*/}
printf '%s %s\n' "$manager" "$*" >> "$TEST_FIXTURE/calls"
[ -f "$ALC_INSTALL_DIR/alc" ] || { printf 'package command before alc install\n' >&2; exit 92; }
if IFS= read -r line; then printf 'package command consumed stdin\n' >&2; exit 91; fi
if [ "$manager $*" = 'brew list --versions tmux' ]; then
  [ "$TEST_BREW_INSTALLED" = 1 ] || exit 1
  printf 'tmux 3.1\n'
  exit 0
fi
[ "$TEST_MANAGER_EXIT" = 0 ] || exit "$TEST_MANAGER_EXIT"
if [ "$manager $*" != 'apt-get update' ]; then
  cp "$TEST_FIXTURE/tmux-stub" "$TEST_FIXTURE/bin/tmux"
  printf '%s\n' "$TEST_AFTER_VERSION" > "$TEST_FIXTURE/version"
  printf '0\n' > "$TEST_FIXTURE/tmux-exit"
fi
STUB
  chmod +x "$fixture/bin/$1"
}

with_sudo() {
  printf '#!%s\n' "$shell" > "$fixture/bin/sudo"
  cat >> "$fixture/bin/sudo" <<'STUB'
printf 'sudo %s\n' "$*" >> "$TEST_FIXTURE/calls"
case "$*" in
  '-n -v')
    if IFS= read -r line; then printf 'sudo probe consumed stdin\n' >&2; exit 91; fi
    [ "$TEST_CACHED_SUDO" = 1 ] ;;
  '-v')
    [ -t 0 ] || { printf 'sudo prompt did not read a terminal\n' >&2; exit 93; }
    [ "$TEST_SUDO_AUTH" = 1 ] || exit 1
    : > "$TEST_FIXTURE/authenticated" ;;
  *)
    [ "$1" = '-n' ] || exit 94
    if IFS= read -r line; then printf 'sudo consumed stdin\n' >&2; exit 91; fi
    [ "$TEST_CACHED_SUDO" = 1 ] || [ -f "$TEST_FIXTURE/authenticated" ] || exit 1
    shift
    "$@" ;;
esac
STUB
  chmod +x "$fixture/bin/sudo"
}

run_installer() {
  result=0
  set -- "$shell"
  if [ "$terminal" != none ]; then
    [ -n "$python" ] || fail 'python3 is required for controlling-terminal tests'
    set -- "$python" "$test_dir/terminal.py" "$shell" "$terminal"
  fi
  env -i PATH="$fixture/bin" HOME="$fixture/home" TMPDIR="$fixture/tmp" SHELL="$shell" \
    ALC_INSTALL_DIR="$fixture/home/bin" ALC_NO_PATH_UPDATE="$no_path" ALC_NO_TMUX_INSTALL="$no_tmux" \
    TEST_FIXTURE="$fixture" TEST_OS="$os" TEST_UID="$uid" TEST_AFTER_VERSION="$after_version" \
    TEST_MANAGER_EXIT="$manager_exit" TEST_CACHED_SUDO="$cached_sudo" TEST_SUDO_AUTH="$sudo_auth" TEST_BAD_CHECKSUM="$bad_checksum" \
    TEST_BREW_INSTALLED="$brew_installed" TEST_ARCHIVE="$test_dir/release.tar.gz" TEST_DIGEST="$digest" \
    "$@" < "$test_dir/install.sh" > "$fixture/output" 2>&1 || result=$?
  [ "$result" = 0 ] || fail "installer exited $result"
  [ -x "$fixture/home/bin/alc" ] || fail 'alc was not installed'
  [ ! -f "$fixture/home/.profile" ] || fail 'opted-out PATH changed'
  assert_output '^script completed$'
  assert_no_output 'consumed stdin'
}

# Missing the dependency phase, or choosing the wrong package arguments, fails
# these full-installer tests; fixture success alone cannot satisfy them.
new_case missing-apt
with_manager apt-get
run_installer
assert_calls 'apt-get update
apt-get install -y tmux'
assert_output 'tmux.*ready.*--tmux'
pass

for version in 'tmux 3.2' 'tmux 3.2a' 'tmux 3.10' 'tmux 4.0'; do
  new_case "compatible-$(printf '%s' "$version" | tr ' ' '-')"
  with_tmux "$version"
  with_manager apt-get
  run_installer
  assert_calls ''
  assert_output 'tmux.*ready.*--tmux'
  pass
done

for version in 'tmux 3.1c' 'tmux unknown' '' 'tmux 4294967296.0' 'tmux 3.4294967296' 'tmux 2.99'; do
  new_case "upgrade-$(printf '%s' "$version" | tr ' ' '-')"
  with_tmux "$version"
  with_manager apt-get
  run_installer
  assert_calls 'apt-get update
apt-get install -y tmux'
  assert_output 'tmux.*ready.*--tmux'
  pass
done

new_case failed-version-command
with_tmux 'tmux 3.6'
printf '1\n' > "$fixture/tmux-exit"
with_manager apt-get
run_installer
assert_calls 'apt-get update
apt-get install -y tmux'
pass

new_case opt-out
no_tmux=1
with_manager apt-get
run_installer
assert_calls ''
assert_output 'ALC_NO_TMUX_INSTALL=1'
pass

new_case no-manager
run_installer
assert_output '[Ww]arning.*tmux'
assert_output 'apt-get.*install.*tmux'
assert_no_output 'tmux.*ready.*--tmux'
pass

new_case failed-apt-update
with_manager apt-get
manager_exit=42
run_installer
assert_calls 'apt-get update'
assert_output '[Ww]arning.*tmux'
assert_no_output 'tmux.*ready.*--tmux'
pass

new_case failed-manager-but-usable-after-reprobe
with_manager dnf
# A package manager can finish installing before a later step fails.
printf '\nexit 42\n' >> "$fixture/bin/dnf"
run_installer
assert_calls 'dnf install -y tmux'
assert_output '[Ww]arning.*tmux'
assert_output 'tmux.*ready.*--tmux'
pass

new_case failed-manager
with_manager dnf
manager_exit=42
run_installer
assert_calls 'dnf install -y tmux'
assert_output '[Ww]arning.*tmux'
assert_output 'dnf install -y tmux'
assert_no_output 'tmux.*ready.*--tmux'
pass

new_case still-missing-after-install
with_manager dnf
# Simulate a successful command that does not put tmux on PATH.
printf '\nrm -f "$TEST_FIXTURE/bin/tmux"\n' >> "$fixture/bin/dnf"
run_installer
assert_output '[Ww]arning.*tmux'
assert_output 'PATH'
assert_no_output 'tmux.*ready.*--tmux'
pass

new_case still-unparseable-after-install
with_manager dnf
after_version='tmux unknown'
run_installer
assert_output '[Ww]arning.*tmux'
assert_no_output 'tmux.*ready.*--tmux'
pass

new_case still-old-after-install
with_tmux 'tmux 3.1'
with_manager apt-get
after_version='tmux 3.1'
run_installer
assert_output '[Ww]arning.*tmux'
assert_output 'PATH'
assert_no_output 'tmux.*ready.*--tmux'
pass

for manager in dnf yum pacman zypper apk; do
  new_case "missing-$manager"
  with_manager "$manager"
  run_installer
  case "$manager" in
    dnf) assert_calls 'dnf install -y tmux' ;;
    yum) assert_calls 'yum install -y tmux' ;;
    pacman) assert_calls 'pacman -S --needed --noconfirm tmux' ;;
    zypper) assert_calls 'zypper --non-interactive install tmux' ;;
    apk) assert_calls 'apk add --upgrade tmux' ;;
  esac
  assert_output 'tmux.*ready.*--tmux'
  pass
done

new_case mac-brew-install
os=Darwin
with_manager brew
run_installer
assert_calls 'brew list --versions tmux
brew install tmux'
assert_output 'tmux.*ready.*--tmux'
pass

new_case mac-brew-upgrade
os=Darwin brew_installed=1
with_tmux 'tmux 3.1c'
with_manager brew
run_installer
assert_calls 'brew list --versions tmux
brew upgrade tmux'
assert_output 'tmux.*ready.*--tmux'
pass

new_case mac-no-brew
os=Darwin
run_installer
assert_calls ''
assert_output '[Ww]arning.*[Hh]omebrew'
assert_output '^  brew install tmux$'
pass

new_case mac-brew-failure
os=Darwin manager_exit=42
with_manager brew
run_installer
assert_output '[Ww]arning.*tmux'
assert_no_output 'tmux.*ready.*--tmux'
pass

new_case cached-sudo
uid=1000
with_manager apt-get
with_sudo
run_installer
assert_calls 'sudo -n -v
sudo -n apt-get update
apt-get update
sudo -n apt-get install -y tmux
apt-get install -y tmux'
assert_output 'tmux.*ready.*--tmux'
pass

new_case no-sudo
uid=1000
with_manager apt-get
run_installer
assert_calls ''
assert_output '[Ww]arning.*sudo'
pass

new_case sudo-without-terminal
uid=1000 cached_sudo=0
with_manager apt-get
with_sudo
run_installer
assert_calls 'sudo -n -v'
assert_output '[Ww]arning.*(sudo|terminal|privilege)'
assert_no_output 'tmux.*ready.*--tmux'
pass

for channel in stdout stderr; do
  new_case "sudo-prompt-$channel"
  uid=1000 cached_sudo=0 terminal=$channel
  with_manager dnf
  with_sudo
  run_installer
  assert_calls 'sudo -n -v
sudo -v
sudo -n dnf install -y tmux
dnf install -y tmux'
  assert_output 'tmux.*ready.*--tmux'
  pass
done

new_case sudo-controlling-tty-but-output-redirected
uid=1000 cached_sudo=0 terminal=hidden
with_manager dnf
with_sudo
run_installer
assert_calls 'sudo -n -v'
assert_output '[Ww]arning.*(sudo|terminal|privilege)'
pass

new_case sudo-authentication-failure
uid=1000 cached_sudo=0 terminal=stdout sudo_auth=0
with_manager dnf
with_sudo
run_installer
assert_calls 'sudo -n -v
sudo -v'
assert_output '[Ww]arning.*sudo authentication failed'
pass

new_case failed-sudo-package-command
uid=1000
with_manager dnf
with_sudo
manager_exit=42
run_installer
assert_calls 'sudo -n -v
sudo -n dnf install -y tmux
dnf install -y tmux'
assert_output '[Ww]arning.*tmux'
assert_no_output 'tmux.*ready.*--tmux'
pass

new_case mac-no-sudo
os=Darwin uid=1000
with_manager brew
with_sudo
run_installer
assert_calls 'brew list --versions tmux
brew install tmux'
pass

new_case checksum-before-dependencies
with_manager apt-get
bad_checksum=1
if (run_installer) > "$fixture/failure" 2>&1; then
  fail 'corrupted archive was accepted'
fi
[ ! -e "$fixture/home/bin/alc" ] || fail 'alc was installed despite checksum mismatch'
assert_calls ''
assert_output 'checksum mismatch'
pass

printf 'Unix installer offline tests passed: %s\n' "$passed"
