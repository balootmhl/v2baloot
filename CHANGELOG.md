# Changelog

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

### Known limitations
- TUN mode, the tray, the system proxy and the Arch package have not been tested on a real Omarchy install yet.
- Requires Xray-core 26.x. Xray rejects `allowInsecure` and unencrypted VLESS to public IPs.
- Only one user per machine can use TUN mode at a time.
