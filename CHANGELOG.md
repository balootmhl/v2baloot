# Changelog

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
