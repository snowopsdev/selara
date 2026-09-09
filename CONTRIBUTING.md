# Contributing to Selara

Thanks for your interest in contributing. This document covers how to build, test, and open pull requests.

## Start here

1. **Clone** — `git clone https://github.com/snowopsdev/selara.git && cd selara`
2. **Test** — `cargo test --workspace` (on macOS also: `cd apps/selara-desktop && npx tauri dev` for Settings)
3. **PR** — branch from `main`, open a PR using the five-section template, wait for CI

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

macOS Settings tray (one-liner, starts the Vite dev server the debug build loads from):

```bash
cd apps/selara-desktop && npx tauri dev
```

(`serve` / hotkeys still need a separate `cargo run -p selara -- serve`.)

CI on GitHub Actions runs four jobs: fmt/clippy/test on Linux (`ubuntu-latest`, excluding the Tauri `selara-desktop` package), a `macos-latest` job that builds and lints the `selara` CLI/`serve` shell and `selara-platform` so the `cfg(target_os = "macos")` code is at least compiled, a check that `apps/selara-desktop/dist/index.html` matches `npm run build`, and the unit tests for the Python helpers under `.cursor/skills/e2e-qa-orchestrator`. Clippy is currently **not** run with `-D warnings` because of known noise (including `objc`-related `cfg` warnings on macOS code paths and a few existing lints). Prefer leaving the tree warning-clean when you can.

Dependency upkeep is automated: Dependabot opens weekly PRs for Cargo, npm (`apps/selara-desktop`), and GitHub Actions, grouping minor and patch bumps into one PR per ecosystem (`.github/dependabot.yml`). `cargo audit` runs in CI against the RustSec advisory database on every `Cargo.toml`/`Cargo.lock` change and on a weekly schedule (`.github/workflows/audit.yml`). `rust-toolchain.toml` pins the `stable` channel with `rustfmt` and `clippy`, so local builds and CI use the same toolchain.

### Platform notes

| Area | CI (Linux) | CI (macOS) | Local macOS |
| --- | --- | --- | --- |
| `selara-core` | Tested | Not run | Covered |
| `selara-platform` traits | Tested | Built + clippy | Covered |
| macOS Accessibility / hotkey / clipboard backends | cfg-gated out | Compiled only; no AX at runtime | Needs local testing |
| `selara` CLI | Tested | Built | Covered |
| `selara` `serve` (egui picker) | cfg-gated out | Compiled only | Needs local macOS testing |
| `selara-desktop` (Tauri) | `dist/index.html` freshness only | Not built | Needs local macOS testing |

If you change hotkeys, selection replace, Accessibility behavior, or the Settings UI, please verify on macOS locally and attach screenshots when UI changes.

## Pull request flow

1. Branch from `main` (`git checkout -b your-topic`)
2. Make focused commits; keep secrets out of the diff
3. Open a PR against `main`. The title must be a conventional commit subject such as `feat(desktop): Add model picker`, because the repo squash-merges and the title becomes the commit on `main` that decides the next version. The description must use the five sections from `.github/PULL_REQUEST_TEMPLATE.md` (What Problem This Solves, Why This Change Was Made, User Impact, Developer Impact, Evidence). A CI check fails the PR if any section is missing or empty, and this applies whether the PR is written by hand, by an IDE, or by a coding agent.
4. Wait for CI to go green
5. **Codex review** — maintainers request this on PRs (GitHub Codex connector). External contributors do **not** need to run Codex themselves; it is not a blocker on your side.
6. Address feedback, then merge when approved

## Releases and versioning

Versions follow [semantic versioning](https://semver.org) and are cut automatically by [release-please](https://github.com/googleapis/release-please) from commit messages, so nobody edits version numbers by hand.

- Pull requests are squash-merged, so the PR title (not the branch commits) is what release-please reads. Keep titles in the `type(scope): Subject` form; CI rejects other titles.
- Commit types decide the bump: `feat` raises the minor version, `fix` and `perf` raise the patch version, and a `!` after the type or a `BREAKING CHANGE:` footer raises the major version. While Selara is `0.x`, a breaking change raises the minor version instead.
- After every merge to `main`, the Release workflow keeps one pull request open titled `chore(main): release X.Y.Z`. It bumps the workspace version in `Cargo.toml`, the desktop `package.json` and `tauri.conf.json`, refreshes `Cargo.lock`, and updates `CHANGELOG.md`.
- Merging that pull request creates the `vX.Y.Z` tag and the GitHub Release, and attaches a macOS build of the `selara` CLI.
- All crates share the workspace version (`version.workspace = true`). Do not set a crate version by hand.
- Commits whose type is `build`, `ci`, `chore`, `test`, `style`, `meta`, or `license` do not appear in the changelog and do not trigger a release on their own.

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
