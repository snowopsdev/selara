# Contributing to Selara

Thanks for your interest in contributing. This document covers how to build, test, and open pull requests.

## Start here

1. **Clone** — `git clone https://github.com/snowopsdev/selara.git && cd selara`
2. **Test** — `cargo test --workspace` (on macOS also: `cargo run -p selara-desktop` for Settings)
3. **PR** — branch from `main`, open a PR with the template, wait for CI

## Prerequisites

- Rust stable (edition 2021 workspace)
- On macOS: Xcode command-line tools (Accessibility / hotkey / egui `serve` shell)
- For the Tauri Settings app (`apps/selara-desktop`): Node.js + a package manager, plus platform-specific Tauri dependencies

## Build and test

From the repo root:

```bash
cargo fmt
cargo clippy --workspace --all-targets
cargo test --workspace
```

macOS Settings tray (one-liner):

```bash
cargo run -p selara-desktop
```

(`serve` / hotkeys still need a separate `cargo run -p selara -- serve`.)

CI on GitHub Actions runs fmt/clippy/test on Linux (`ubuntu-latest`), excluding the Tauri `selara-desktop` package. Clippy is currently **not** run with `-D warnings` because of known noise (including `objc`-related `cfg` warnings on macOS code paths and a few existing lints). Prefer leaving the tree warning-clean when you can.

### Platform notes

| Area | CI (Linux) | Local macOS |
| --- | --- | --- |
| `selara-core` | Covered | Covered |
| `selara-platform` traits | Covered | Covered |
| macOS Accessibility / hotkey / clipboard backends | Compile stubs / cfg-gated; no real AX coverage | Needs local testing |
| `selara` CLI | Covered | Covered |
| `selara` `serve` (egui picker) | Not exercised on Linux | Needs local macOS testing |
| `selara-desktop` (Tauri) | Excluded from Linux CI | Needs local macOS testing |

If you change hotkeys, selection replace, Accessibility behavior, or the Settings UI, please verify on macOS locally and attach screenshots when UI changes.

## Pull request flow

1. Branch from `main` (`git checkout -b your-topic`)
2. Make focused commits; keep secrets out of the diff
3. Open a PR against `main` (use the PR template)
4. Wait for CI to go green
5. **Codex review** — maintainers request this on PRs (GitHub Codex connector). External contributors do **not** need to run Codex themselves; it is not a blocker on your side.
6. Address feedback, then merge when approved

## No secrets

Do **not** commit:

- API keys (`SELARA_API_KEY`, provider keys in `config.toml`, etc.)
- `.env` files, `*.pem`, private keys, tokens
- Local Codex / ChatGPT auth under `~/.codex` or copied auth files
- Personal `config.toml` with credentials

Use env vars or local-only config ignored by git.

## Code of conduct

By participating, you agree to uphold our [Code of Conduct](CODE_OF_CONDUCT.md).

## License

Contributions are licensed under the [MIT License](LICENSE).
