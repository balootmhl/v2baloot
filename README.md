# v2baloot

Omarchy (Arch Linux + Hyprland) အတွက် Xray (V2Ray) client တစ်ခုပါ။ Desktop GUI၊ tray icon၊ terminal UI၊ Waybar indicator နဲ့ Walker menu တို့ကို ပေါင်းပေးထားပါတယ်။ အားလုံးက daemon တစ်ခုတည်းကို ချိတ်သုံးကြလို့ GUI ပိတ်လိုက်လည်း connection ဆက်ရှိနေပါတယ်။

An Xray client for Omarchy: one background daemon owns the connection, and the GTK4 window, tray icon, TUI, Waybar module, Walker menu and CLI all talk to it.

| | |
|---|---|
| Protocols | VLESS (incl. REALITY / Vision), VMess, Trojan, Shadowsocks (incl. 2022) |
| Transports | TCP (+HTTP header), WebSocket, gRPC, HTTPUpgrade, XHTTP (HTTP/2 links map to XHTTP stream-one) |
| Modes | **Proxy**: SOCKS5 `127.0.0.1:10808` + HTTP `:10809`, plus system proxy · **TUN**: every app, using Xray's native TUN |
| Servers | Share links, clipboard import, subscriptions (base64 or plain, usage and expiry shown, auto-update) |
| Latency | Real delay test through each server, run in parallel |

## Install (Omarchy)

```bash
git clone https://github.com/balootmhl/v2baloot.git && cd v2baloot
./install.sh
```

`install.sh` does the following:
1. Builds an Arch package with `makepkg -si`. Dependencies: `xray`, `gtk4`, `libadwaita`, `wl-clipboard`, `libnotify`.
2. Runs `systemctl --user enable --now v2baloot` to start the daemon.
3. Asks before adding the Waybar indicator and Hyprland keybindings. It backs up each config file first.

## Usage

**ပထမဆုံး server ထည့်ခြင်း**

```bash
v2baloot sub add 'https://provider.example/sub/xxxx' -n "My VPN"   # subscription
v2baloot import 'vless://…'                                       # or copy the link and run: v2baloot import
v2baloot test                                                     # latency test
v2baloot connect 3        # connect by list number, name, or id
```

| Where | How |
|---|---|
| GUI | Walker → "v2baloot", or `v2baloot-gui` |
| TUI | `v2baloot` (press `?` for keys) |
| Waybar | click = open window · middle = TUI · right = connect/disconnect |
| Tray | Waybar tray: connect, switch servers, mode, test, import |
| Keys | `Super+Alt+V` server menu (Walker) · `Super+Alt+Shift+V` connect/disconnect |
| CLI | `v2baloot status / list / toggle / mode tun / sub update / logs` |

### Proxy mode vs TUN mode

- **Proxy**: v2baloot sets the proxy for GTK apps (gsettings) and for apps you launch afterwards (systemd user environment and Hyprland env). Apps that were already open need a restart. Terminal tools can use `socks5h://127.0.0.1:10808` directly.
- **TUN**: all traffic, including terminal tools and apps that ignore proxy settings, goes through the tunnel (IPv6 too; if it can't be tunnelled, it's blocked rather than leaked). The `v2baloot-tun@<user>.service` system unit sets up routing as root and runs Xray's native TUN as `nobody` with only network capabilities. The polkit rule lets members of `wheel` (the Omarchy default) start it without a password from Waybar, the tray or a keybinding. DNS is also sent through the proxy. Only one user on the machine can use TUN mode at a time.

## Files

| Path | Contents |
|---|---|
| `~/.config/v2baloot/config.toml` | settings (ports, DNS, test URL, tray, menu command…) |
| `~/.local/share/v2baloot/servers.json` | servers and subscriptions (mode 0600, contains credentials) |
| `$XDG_RUNTIME_DIR/v2baloot/` | daemon socket, generated Xray config |

## Troubleshooting

- `v2baloot logs` shows Xray's output. For TUN mode, use `journalctl -u 'v2baloot-tun@*' -n 50`.
- *"vless without TLS … is prohibited"*: Xray-core itself refuses unencrypted VLESS to public IPs. The server needs TLS or REALITY.
- `allowInsecure=1` links: Xray 26 removed that option, so v2baloot ignores it. Self-signed servers need `pcs=` (pinned certificate SHA-256) or `vcn=` in the link, which are passed through.
- For floating windows, add a Hyprland window rule for app id `org.omarchy.v2baloot` (or `org.omarchy.v2baloot.tui`) in the syntax your Hyprland version uses.
- If the daemon won't start: `systemctl --user status v2baloot`.
- If Proxy mode doesn't work in some app: restart that app, or switch to TUN mode.

## Development

```
crates/core      share links, Xray config, storage, subscriptions, latency test, IPC (no GTK)
crates/v2baloot  daemon, CLI, TUI, tray, Waybar, Walker menu, TUN helper
crates/gui       GTK4 / libadwaita window
packaging/       PKGBUILD, systemd units, polkit rule, desktop file, icons, Omarchy snippets
```

```bash
cargo test --workspace            # unit tests + `xray run -test` validation of generated configs
cargo run -p v2baloot -- daemon   # then `cargo run -p v2baloot -- tui` in another terminal
cargo run -p v2baloot-gui
```

The core library, daemon (Proxy mode), TUI and GUI also build and run on macOS (`brew install gtk4 libadwaita xray`) for development. TUN mode, the tray and the system proxy are Linux-only.

`V2BALOOT_SCREENSHOT=out.png v2baloot-gui` renders the window to a PNG and exits. `V2BALOOT_DEBUG_ACTION=win.prefs` opens a dialog first.

License: MIT
