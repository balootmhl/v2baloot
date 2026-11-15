# Changelog

## v0.1.0 — 2026-11-16

First stable release. Everything below plus the betas (installers, kill switch, security hardening, redesigned UI).

### TUN kill switch
- In TUN mode, if Xray stops, internet traffic is blocked instead of leaving outside the tunnel. The root helper restarts Xray on its own (1–30 s backoff), and the app shows "connecting" until it's back. It is on by default; Preferences → Kill switch.

### Installing
- Each release tag now gets a prebuilt Arch package from CI. On a release tag, `install.sh` installs it in seconds after checking its checksum; `--build` builds from source.
- An AUR package `v2baloot-bin` (in `packaging/aur/`).
- CI runs fmt, clippy and tests on every push and pull request.

### Docs
- New README with screenshots, plus SECURITY.md, CONTRIBUTING.md and issue templates.

## v0.1.0-beta.2 — 2026-11-16

### Fixes
- TUN stop no longer hangs for 10 s with DNS broken afterwards. The unit lacked `CAP_KILL`, so the helper could not stop its unprivileged Xray. Cleanup now also resets systemd-resolved's server state.

### Faster installs
- Lighter release profile (no LTO, parallel codegen).
- `makepkg` keeps its build cache in the checkout (`target/makepkg`), so updates only recompile what changed.
- `install.sh` skips compiling the test suite.

### Security
- Text from subscriptions and share links (server names, titles, errors) is stripped of control characters and bidi overrides. This closes a terminal escape injection (e.g. OSC 52 clipboard writes) in the TUI and CLI, and spoofing in notifications and the tray tooltip.
- Subscription downloads are capped at 8 MB and 5000 servers, and parsing runs off the async workers.
- Redirects are limited: no https→http and no redirects to local addresses.
- Servers that point at this machine are ignored when they come from a subscription.
- A warning is shown for `http://` subscription links.
- A changed subscription never silently switches you to a different server.
- "Allow LAN" proxies can no longer reach this machine's loopback services.
- The exit IP lookup uses HTTPS only, with validated and size-capped responses.
- IPC socket:
  - request size limit, idle timeout and connection cap;
  - clients check that the socket belongs to their own user;
  - settings are validated before they are saved.
- Credentials files are always written with mode 0600 in 0700 directories.
- Root helper:
  - Xray runs as a dedicated `v2baloot-tun` system user;
  - it refuses to run Xray as root if `setpriv` is missing;
  - programs are called by absolute path;
  - the journal log level is capped at warning;
  - extra systemd sandboxing (device policy, clock, kernel logs, SUID…).
- `install.sh` no longer passes `--noconfirm` to the AUR helper, so you see the `xray-bin` PKGBUILD before it is installed.

## v0.1.0-beta.1 — 2026-11-15

### New look
- Redesigned window: a dashboard with an animated connect ring, a session timer, a Proxy/TUN switch, and the current server.
- The dashboard also shows live speed with a scrolling graph, plus exit IP and country.
- The server list gets group pills, flags, signal bars and hover menus.
- Narrow windows show the dashboard first and the server list as a second page.

### Fixes
- The tray icon, notification icon and Waybar icon are now rendered by v2baloot itself in the Omarchy theme colours. They no longer show as a blank square or a checkerboard.
- Disconnecting updates every front-end immediately. TUN and system-proxy teardown is much faster: batched `ip` calls, a non-blocking unit stop, and system-proxy commands run in parallel.
- Install `xray-bin` from the AUR (xray is not in the official repos).

## v0.1.0-alpha.1 — 2026-11-15

First public alpha.

### Features
- **Protocols**: VLESS (REALITY, Vision), VMess, Trojan and Shadowsocks (including 2022 ciphers).
- **Transports**: TCP, WebSocket, gRPC, HTTPUpgrade and XHTTP. HTTP/2 links map to XHTTP stream-one.
- **Servers**: import share links from the clipboard. Subscriptions (base64 or plain) show usage and expiry and update automatically.
- **Latency test**: real delay test through every server, in parallel.
- **Proxy mode**: local SOCKS5/HTTP proxy, plus system proxy (gsettings, systemd user environment, Hyprland env).
- **TUN mode**: routes every app through Xray's native TUN, started through a hardened root unit and a polkit rule.
- **Front-ends**: GTK4/libadwaita window that follows the Omarchy theme, ratatui TUI, tray icon, Waybar module, Walker server menu and CLI.
- **Connectivity**: connection timeouts match V2Box. The daemon reconnects automatically with backoff and restarts the connection after resume from sleep.

### Look
- New "b-tunnel" app icon: a tunnel whose tall left wall and floor form a "b".
- Matching symbolic tray icons for connected, disconnected, TUN and error.
- The Waybar module shows the same logo, drawn in the bar's text colour.

### Known limitations
- TUN mode, the tray, the system proxy and the Arch package have not been tested on a real Omarchy install yet.
- Requires Xray-core 26.x. Xray rejects `allowInsecure` and unencrypted VLESS to public IPs.
- Only one user per machine can use TUN mode at a time.
