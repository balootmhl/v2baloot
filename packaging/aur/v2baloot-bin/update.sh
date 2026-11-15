#!/usr/bin/env bash
# Bump the AUR package to a released version. Run on Arch Linux:
#   ./update.sh 0.1.0
# then test with `makepkg -si`, and publish (see packaging/aur/README.md).
set -euo pipefail
cd "$(dirname "$0")"
ver="${1:?usage: ./update.sh <version, e.g. 0.1.0>}"
[[ $ver =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "AUR releases are stable versions only (x.y.z)" >&2; exit 1; }
file="v2baloot-$ver-1-x86_64.pkg.tar.zst"
sum=$(curl -fsSL --proto '=https' "https://github.com/balootmhl/v2baloot/releases/download/v$ver/sha256sums.txt" |
  awk -v f="$file" '$2 == f { print $1 }')
[[ $sum =~ ^[0-9a-f]{64}$ ]] || { echo "no checksum for $file in the v$ver release" >&2; exit 1; }
sed -i -e "s/^pkgver=.*/pkgver=$ver/" -e "s/^pkgrel=.*/pkgrel=1/" -e "s/^sha256sums=.*/sha256sums=('$sum')/" PKGBUILD
makepkg --printsrcinfo > .SRCINFO
echo "PKGBUILD and .SRCINFO updated to $ver ($sum)."
echo "Next: makepkg -si   (test)   then publish — see ../README.md"
