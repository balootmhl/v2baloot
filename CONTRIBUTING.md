# Contributing

Thanks for helping! Bug reports and pull requests are welcome.

## Branches

- `main` — released code; every release is a tag on `main`.
- `develop` — integration branch; open pull requests against `develop`.
- `feature/<name>` — new work, branched from `develop`.
- `hotfix/<name>` — urgent fixes, branched from `main`, merged into `main` and `develop`.

## Before opening a pull request

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

CI runs the same checks on Arch Linux. Please describe how you tested the change — TUN mode,
the tray and the system proxy can only be tested on Linux.

Security problems: see [SECURITY.md](SECURITY.md) instead of opening an issue.
