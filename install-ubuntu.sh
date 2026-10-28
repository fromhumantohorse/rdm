#!/bin/sh
# RDM on Ubuntu 26.04, in one command:      sh install-ubuntu.sh
# Uninstall:                                sh install-ubuntu.sh --uninstall
#
# 1. installs the build tools and, if needed, Rust (rustup, in ~/.cargo);
# 2. builds an optimized RDM;
# 3. installs it for your account (~/.local, no administrator rights for RDM itself):
#    in the applications menu (GNOME, KDE…) with its icon, started at every login
#    (minimized to the notification area);
# 4. starts it.
# sudo is only used for missing system packages (apt).
set -eu
cd "$(dirname "$0")"

# Run from a Flatpak app's terminal (VSCodium, VS Code…), the script would build against the
# sandbox's libraries and install into the app's private folders (~/.var/app/…), where GNOME
# never looks: no icon in the applications. Run it on the real system instead.
if [ -f /.flatpak-info ]; then
    if command -v flatpak-spawn >/dev/null; then
        printf '\033[1;35m==>\033[0m %s\n' "Flatpak sandbox detected: running on the host system"
        exec flatpak-spawn --host sh "$(pwd)/install-ubuntu.sh" "$@"
    fi
    printf '\033[31merror:\033[0m %s\n' "run this script from Ubuntu's Terminal, not from a Flatpak app" >&2
    exit 1
fi

# Run with sudo, RDM would be installed for root (/root/.local): no menu entry, no icon, no
# autostart in the user's session, and a root-owned target/. Go on as the real user instead.
if [ "$(id -u)" -eq 0 ]; then
    if [ -n "${SUDO_USER:-}" ] && [ "$SUDO_USER" != root ]; then
        printf '\033[1;35m==>\033[0m %s\n' "do not use sudo: going on as $SUDO_USER"
        exec sudo -u "$SUDO_USER" -H sh "$(pwd)/install-ubuntu.sh" "$@"
    fi
    printf '\033[31merror:\033[0m %s\n' "run this script as your normal user, without sudo" >&2
    exit 1
fi

say() { printf '\033[1;35m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[33mwarning:\033[0m %s\n' "$*" >&2; }
# sudo needs a terminal for the password; without one (started from a Flatpak app), pkexec asks
# in a window.
as_root() {
    if [ -t 0 ] || sudo -n true 2>/dev/null || ! command -v pkexec >/dev/null; then sudo "$@"
    else pkexec "$@"
    fi
}

if [ "${1:-}" = "--uninstall" ]; then
    exec sh packaging/linux/install.sh --uninstall
fi

if [ -r /etc/os-release ]; then
    # shellcheck disable=SC1091
    . /etc/os-release
fi
case " ${ID:-} ${ID_LIKE:-} " in *" ubuntu "* | *" debian "*) ;; *) false ;; esac || warn "made for Ubuntu 26.04 (this system: ${PRETTY_NAME:-unknown}); going on"

# sudo only when a package is really missing: a refused or cancelled password must not stop the
# install before the icon is created.
missing=""
for pkg in build-essential pkg-config curl ca-certificates; do
    dpkg -s "$pkg" >/dev/null 2>&1 || missing="$missing $pkg"
done
if [ -n "$missing" ]; then
    say "Build tools:$missing (sudo may ask for your password)"
    # shellcheck disable=SC2086
    { as_root apt-get update -qq &&
        as_root env DEBIAN_FRONTEND=noninteractive apt-get install -y -qq $missing >/dev/null; } ||
        warn "could not install$missing; going on"
fi

# Rust: the project's minimum version (rust-version in Cargo.toml), through rustup.
need=$(sed -n 's/^rust-version *= *"\([0-9.]*\)"/\1/p' Cargo.toml)
# `cargo 1.90.0 (…)` → true if at least $need; false when cargo is missing.
recent_enough() {
    version=$("$1" --version 2>/dev/null | cut -d' ' -f2)
    [ -n "$version" ] || return 1
    major=${version%%.*} rest=${version#*.}
    minor=${rest%%.*}
    need_major=${need%%.*} need_rest=${need#*.}
    need_minor=${need_rest%%.*}
    [ "$major" -gt "$need_major" ] || { [ "$major" -eq "$need_major" ] && [ "$minor" -ge "$need_minor" ]; }
}
cargo=$(command -v cargo || echo "$HOME/.cargo/bin/cargo")
if ! recent_enough "$cargo"; then
    if [ -x "$HOME/.cargo/bin/rustup" ]; then
        say "Updating Rust"
        "$HOME/.cargo/bin/rustup" update stable
    else
        say "Installing Rust (rustup, in ~/.cargo)"
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
    fi
    cargo="$HOME/.cargo/bin/cargo"
fi

# target/ left owned by root by an earlier `sudo` run: build in the user's cache instead.
target=${CARGO_TARGET_DIR:-target}
if [ -e "$target" ] && find "$target" -maxdepth 2 ! -writable -print -quit 2>/dev/null | grep -q .; then
    warn "$target is not writable (an earlier run with sudo?): building in ~/.cache/rdm-target; 'sudo rm -rf $(pwd)/target' cleans it"
    target="${XDG_CACHE_HOME:-$HOME/.cache}/rdm-target"
fi
export CARGO_TARGET_DIR="$target"

say "Building RDM (a few minutes the first time)"
"$cargo" build --release -p rdm
# A binary built elsewhere (another toolchain, a Flatpak SDK) may need a newer glibc: rebuild it.
if ldd "$target/release/rdm" 2>&1 | grep -q 'not found'; then
    say "Rebuilding RDM for this system"
    "$cargo" clean --release -p rdm
    "$cargo" build --release -p rdm
fi

say "Installing: applications menu, icon, start at login"
sh packaging/linux/install.sh --autostart

# An RDM already open was closed cleanly then started again by install.sh (or reported if it is
# too old to close on request). The browsers' connectors run the same binary: not counted.
if ! pgrep -a -x rdm 2>/dev/null | grep -v -e 'chrome-extension://' -e 'rdm@rdm-download-manager' -e 'rdm.bridge.json' | grep -q .; then
    say "Starting RDM"
    nohup "$HOME/.local/bin/rdm" >/dev/null 2>&1 &
fi
echo
echo "RDM is installed: look for \"RDM\" in the applications (Super key)."
echo "Browser extension: in RDM, the \"Browser extension\" card (Firefox, Waterfox, Chrome, Brave, Opera, Edge)."
