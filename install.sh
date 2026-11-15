#!/usr/bin/env bash
# v2baloot installer for Omarchy / Arch Linux.
#
#   ./install.sh              install the package (prebuilt when on a release tag), enable the daemon,
#                             offer Omarchy integration
#   ./install.sh --build      always build from source instead of using a prebuilt package
#   ./install.sh --integrate  only add the Waybar module and Hyprland keybindings
#   ./install.sh --yes        don't ask before editing Waybar/Hyprland config
set -euo pipefail

cd "$(dirname "$0")"
ROOT="$PWD"
ASSUME_YES=0
ONLY_INTEGRATE=0
FORCE_BUILD=0
REPO_URL="https://github.com/balootmhl/v2baloot"
for a in "$@"; do
  case "$a" in
    --build) FORCE_BUILD=1 ;;
    --yes|-y) ASSUME_YES=1 ;;
    --integrate) ONLY_INTEGRATE=1 ;;
    -h|--help) sed -n '2,9p' "$0"; exit 0 ;;
    *) echo "unknown option: $a" >&2; exit 2 ;;
  esac
done

say() { printf '\033[1;34m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m==>\033[0m %s\n' "$*"; }
ask() {
  [[ $ASSUME_YES == 1 ]] && return 0
  read -r -p "$1 [Y/n] " r
  [[ -z $r || $r =~ ^[Yy] ]]
}
backup() { cp -a "$1" "$1.bak.$(date +%Y%m%d%H%M%S)"; }

if [[ ! -f /etc/arch-release ]]; then
  echo "This installer is for Arch Linux / Omarchy." >&2
  exit 1
fi

ensure_xray() {
  # xray is not in the official repos, only in the AUR. xray-bin (official release binary,
  # provides `xray`) avoids a Go build.
  pacman -T xray >/dev/null && return 0
  local aur=""
  for h in yay paru; do
    if command -v "$h" >/dev/null; then aur=$h; break; fi
  done
  if [[ -z $aur ]]; then
    echo "xray is only available in the AUR. Install it first (e.g. 'yay -S xray-bin'), then re-run." >&2
    exit 1
  fi
  say "Installing xray from the AUR (xray-bin) with $aur…"
  # No --noconfirm: the AUR helper shows the PKGBUILD for review — its xray binary later
  # runs under a root unit.
  "$aur" -S --needed xray-bin
}

# On a release tag, install the package CI built for it: seconds instead of a Rust build.
install_prebuilt() {
  [[ $FORCE_BUILD == 0 && $(uname -m) == x86_64 ]] || return 1
  local tag
  tag=$(git -C "$ROOT" describe --tags --exact-match 2>/dev/null) || return 1
  [[ $tag =~ ^v[0-9A-Za-z.+-]+$ ]] || return 1
  local base="$REPO_URL/releases/download/$tag" tmp pkg
  tmp=$(mktemp -d)
  # shellcheck disable=SC2064  # expand now: $tmp is local
  trap "rm -rf '$tmp'" EXIT
  say "Downloading the prebuilt package for $tag…"
  curl -fsSL --proto '=https' "$base/sha256sums.txt" -o "$tmp/sha256sums.txt" || {
    warn "No prebuilt package for $tag; building from source."
    return 1
  }
  pkg=$(awk '{print $2}' "$tmp/sha256sums.txt" | grep -m1 -E '^v2baloot-[0-9A-Za-z.+_-]+-x86_64\.pkg\.tar\.zst$') || return 1
  curl -fsSL --proto '=https' "$base/$pkg" -o "$tmp/$pkg" || return 1
  if ! (cd "$tmp" && grep -F " $pkg" sha256sums.txt | sha256sum -c --quiet -); then
    warn "Checksum mismatch for $pkg; building from source instead."
    return 1
  fi
  sudo pacman -U --noconfirm "$tmp/$pkg"
}

build_from_source() {
  say "Building the v2baloot package from source (makepkg)…"
  command -v cargo >/dev/null || sudo pacman -S --needed --noconfirm rust
  # No --needed: a rebuild with the same pkgver must still replace the installed binaries.
  # --nocheck: skip compiling the test suite (CI runs it); the build cache in target/makepkg
  # makes later updates incremental.
  (cd packaging && makepkg -si --noconfirm --nocheck)
  rm -rf packaging/src packaging/pkg packaging/*.pkg.tar.zst
}

install_package() {
  ensure_xray
  install_prebuilt || build_from_source
  say "Enabling the user daemon…"
  systemctl --user daemon-reload
  systemctl --user enable --now v2baloot.service
  systemctl --user restart v2baloot.service
}

integrate_hyprland() {
  local conf="$HOME/.config/hypr/hyprland.conf"
  local line="source = /usr/share/v2baloot/hyprland.conf"
  [[ -f $conf ]] || { warn "No $conf; skipping Hyprland."; return; }
  if grep -qF "$line" "$conf"; then
    say "Hyprland already sources v2baloot."
    return
  fi
  if ask "Add keybindings (Super+Alt+V menu, Super+Alt+Shift+V toggle) to Hyprland?"; then
    backup "$conf"
    printf '\n# v2baloot\n%s\n' "$line" >>"$conf"
    hyprctl reload >/dev/null 2>&1 || true
    say "Hyprland updated."
  fi
}

# Write (or replace) the v2baloot block in Waybar's style.css, pointing at the icons the daemon
# renders under $XDG_CACHE_HOME. Older installs without markers are cleaned up too.
install_waybar_css() {
  local css="$1" cache="${XDG_CACHE_HOME:-$HOME/.cache}/v2baloot"
  [[ -f $css ]] || return 0
  command -v python3 >/dev/null || { warn "python3 not found; add /usr/share/v2baloot/waybar.css by hand."; return 0; }
  backup "$css"
  python3 - "$css" /usr/share/v2baloot/waybar.css "$cache" <<'PY'
import re, sys
path, tpl, cache = sys.argv[1:4]
text = open(path).read()
text = re.sub(r"\n?/\* >>> v2baloot >>>.*?<<< v2baloot <<< \*/\n?", "\n", text, flags=re.S)
# legacy block (no markers): drop its comment and every #custom-v2baloot rule
text = re.sub(r"/\*[^*]*v2baloot[^*]*\*+(?:[^/*][^*]*\*+)*/\n?", "", text)
text = re.sub(r"[^{}]*#custom-v2baloot[^{}]*\{[^}]*\}\n?", "", text)
block = open(tpl).read().replace("@CACHE@", cache)
open(path, "w").write(text.rstrip() + "\n\n" + block)
PY
}

integrate_waybar() {
  local cfg="$HOME/.config/waybar/config.jsonc" css="$HOME/.config/waybar/style.css"
  [[ -f $cfg ]] || { warn "No $cfg; skipping Waybar."; return; }
  if grep -q '"custom/v2baloot"' "$cfg"; then
    say "Waybar already has the v2baloot module; refreshing its style."
    install_waybar_css "$css"
    return
  fi
  ask "Add the v2baloot indicator to Waybar?" || return 0
  command -v python3 >/dev/null || { warn "python3 not found; add /usr/share/v2baloot/waybar-module.jsonc by hand."; return; }
  backup "$cfg"
  python3 - "$cfg" /usr/share/v2baloot/waybar-module.jsonc <<'PY'
import re, sys
path, module_path = sys.argv[1], sys.argv[2]
text = open(path).read()
module = "\n".join(l for l in open(module_path).read().splitlines() if not l.lstrip().startswith("//"))
new, n = re.subn(r'("modules-right"\s*:\s*\[)', r'\1\n    "custom/v2baloot",', text, count=1)
if n == 0:
    sys.exit("could not find modules-right")
new = new.replace("{", "{\n  " + module.strip().replace("\n", "\n  ") + ",", 1)
open(path, "w").write(new)
PY
  install_waybar_css "$css"
  if command -v omarchy-restart-waybar >/dev/null; then omarchy-restart-waybar >/dev/null 2>&1 || true
  else pkill -SIGUSR2 waybar 2>/dev/null || true; fi
  say "Waybar updated (backup saved next to the config)."
}

if [[ $ONLY_INTEGRATE == 0 ]]; then
  install_package
fi
integrate_hyprland
integrate_waybar

cat <<'EOF'

v2baloot is ready.
  • Open the app from Walker ("v2baloot") or run: v2baloot-gui
  • Terminal UI:       v2baloot
  • Add servers:       v2baloot sub add <url>   |   v2baloot import <link>
  • Super+Alt+V        server menu,  Super+Alt+Shift+V  connect/disconnect
EOF
