#!/usr/bin/env bash
# v2baloot installer for Omarchy / Arch Linux.
#
#   ./install.sh              build + install the package, enable the daemon, offer Omarchy integration
#   ./install.sh --integrate  only add the Waybar module and Hyprland keybindings
#   ./install.sh --yes        don't ask before editing Waybar/Hyprland config
set -euo pipefail

cd "$(dirname "$0")"
ROOT="$PWD"
ASSUME_YES=0
ONLY_INTEGRATE=0
for a in "$@"; do
  case "$a" in
    --yes|-y) ASSUME_YES=1 ;;
    --integrate) ONLY_INTEGRATE=1 ;;
    -h|--help) sed -n '2,7p' "$0"; exit 0 ;;
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

install_package() {
  say "Building and installing the v2baloot package (makepkg)…"
  command -v cargo >/dev/null || sudo pacman -S --needed --noconfirm rust
  # xray is not in the official repos, only in the AUR. xray-bin (official release binary,
  # provides `xray`) avoids a Go build.
  if ! pacman -T xray >/dev/null; then
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
  fi
  # No --needed: a rebuild with the same pkgver must still replace the installed binaries.
  (cd packaging && makepkg -si --noconfirm)
  rm -rf packaging/src packaging/pkg packaging/*.pkg.tar.zst
  say "Enabling the user daemon…"
  systemctl --user daemon-reload
  systemctl --user enable --now v2baloot.service
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
