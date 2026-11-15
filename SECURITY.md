# Security policy

## Supported versions

Only the latest release receives security fixes.

## Reporting a vulnerability

Please **do not open a public issue** for security problems. Report them privately through
GitHub: **Security → Report a vulnerability** on https://github.com/balootmhl/v2baloot.

Include what you found, how to reproduce it, and what an attacker gains. You'll get an answer
within a week; fixes are released as soon as they are ready and credited if you wish.

## Scope

In scope:
- the root TUN helper (`v2baloot tun-run`), its systemd unit and the polkit rule
- the user daemon and its IPC socket
- handling of untrusted input: share links, subscription content and headers, the exit-IP lookup
- the installer and packaging

Out of scope: vulnerabilities in Xray-core itself (report them to
[XTLS/Xray-core](https://github.com/XTLS/Xray-core)), and the documented design trade-offs in the
README's *Security* section (e.g. TUN mode being system-wide).
