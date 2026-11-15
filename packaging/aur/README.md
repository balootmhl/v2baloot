# Publishing `v2baloot-bin` to the AUR

The AUR package repackages the `.pkg.tar.zst` that the Release workflow builds for each stable tag.

One-time setup:
1. Create an account on https://aur.archlinux.org and add your SSH public key in *My Account*.
2. `git clone ssh://aur@aur.archlinux.org/v2baloot-bin.git ~/aur/v2baloot-bin`
   (an empty repo the first time — that reserves the name).

For every stable release (after CI has attached the package to the GitHub release):
```bash
cd ~/v2baloot/packaging/aur/v2baloot-bin
./update.sh 0.1.0          # sets pkgver + sha256, writes .SRCINFO
makepkg -si                # test install
cp PKGBUILD .SRCINFO v2baloot.install ~/aur/v2baloot-bin/
cd ~/aur/v2baloot-bin && git add -A && git commit -m "Update to 0.1.0" && git push
```
Users then install with `yay -S v2baloot-bin` and get updates through `yay -Syu`.
