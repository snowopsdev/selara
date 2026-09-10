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

CI on GitHub Actions runs four jobs: fmt/clippy/test on Linux (`ubuntu-latest`, excluding the Tauri `selara-desktop` package), a `macos-latest` job that builds and lints the `selara` CLI/`serve` shell and `selara-platform` so the `cfg(target_os = "macos")` code is at least compiled, a check that `apps/selara-desktop/dist/index.html` matches `npm run build`, and the unit tests for the Python helpers under `.cursor/skills/e2e-qa-orchestrator`. Clippy runs with `-D warnings`, so any new warning fails CI; the `objc` macro `cfg` noise on macOS code paths is declared through `check-cfg` in `crates/selara-platform/Cargo.toml` rather than allowed.

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
- Merging that pull request creates the `vX.Y.Z` tag and the GitHub Release, and attaches a macOS build of the `selara` CLI, a DMG of the Tauri app, and a rendered Homebrew cask and formula (`homebrew/*.tmpl` via `scripts/release/render-homebrew.sh`). Signing and notarization run when the `APPLE_*` repository secrets are set (see the comment block above the `desktop` job in `.github/workflows/release.yml`); without them the DMG is ad-hoc signed. The cask's Gatekeeper caveat is dropped only when `xcrun stapler validate` finds a notarization ticket on the built DMG, so a signed-but-unnotarized build keeps the workaround. The cask and formula are attached as `selara-cask.rb` and `selara-formula.rb` (both files are named `selara.rb`, and release asset names have to be unique). `HOMEBREW_TAP_TOKEN` enables the push to `snowopsdev/homebrew-selara`.
- All crates share the workspace version (`version.workspace = true`). Do not set a crate version by hand.
- Commits whose type is `build`, `ci`, `chore`, `test`, `style`, `meta`, or `license` do not appear in the changelog and do not trigger a release on their own.

### Desktop auto-update signing

The menu-bar app checks GitHub Releases for a newer build through `tauri-plugin-updater` (Status tab → Updates, or the tray's "Check for updates…"). The plugin only accepts artifacts signed with the project's updater key, and until that key exists `apps/selara-desktop/src-tauri/tauri.conf.json` ships the placeholder `REPLACE_WITH_TAURI_UPDATER_PUBKEY`. The app treats the placeholder as "updates not configured" and never contacts the network, so nothing breaks in the meantime; it just does not update itself.

A maintainer enables it once:

1. Generate the keypair (never inside the repo): `npx tauri signer generate -w ~/.tauri/selara.key` from `apps/selara-desktop`. Choose a password when prompted.
2. Put the printed **public** key into `plugins.updater.pubkey` in `tauri.conf.json` and commit that change.
3. Add the **private** key (`~/.tauri/selara.key` contents) and its password as repository secrets `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`. The release workflow's `desktop` job passes them to `tauri build` and uploads `Selara.app.tar.gz`, its `.sig`, and `latest.json`; see `docs/release-updater.md`.

The private key and password must never be committed or pasted into an issue. Losing the private key means shipping a new public key in a release users must install by hand, so keep a copy somewhere safe.

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
