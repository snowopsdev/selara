# Contributing to Selara

Thanks for your interest in contributing. This document covers how to build, test, and open pull requests.

## Start here

1. **Clone** — `git clone https://github.com/snowopsdev/selara.git && cd selara`
2. **Test** — `cargo test --workspace` (on macOS also: `cd apps/selara-desktop && npx tauri dev` for Settings)
3. **PR** — branch from `main`, open a PR using the five-section template, wait for CI

## Prerequisites

- Rust stable, version 1.95 or newer (edition 2021 workspace; required by the egui shell)
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

CI checks Rust formatting/lints/tests, macOS platform behavior, frontend DOM regressions and committed dist, release helpers, and full app/DMG packaging. Packaging runs both local mode with empty Apple credentials and updater mode with a temporary test key. It verifies every bundled executable and the signed updater archive. The actual pinned writing runtime runs hostile-home isolation and authentication race tests. Python 3.12 and Rust 1.95.0 are required for that native runtime; its build is locked and cached. Interactive Accessibility and production notarization still require a local/release check.

Dependency upkeep is automated: Dependabot opens weekly PRs for Cargo, npm (`apps/selara-desktop`), and GitHub Actions, grouping minor and patch bumps into one PR per ecosystem (`.github/dependabot.yml`). `cargo audit` runs in CI against the RustSec advisory database on every `Cargo.toml`/`Cargo.lock` change and on a weekly schedule (`.github/workflows/audit.yml`). `rust-toolchain.toml` pins the `stable` channel with `rustfmt` and `clippy`, so local builds and CI use the same toolchain.

### Platform notes

| Area | CI (Linux) | CI (macOS) | Local macOS |
| --- | --- | --- | --- |
| `selara-core` | Tested | Not run | Covered |
| `selara-platform` traits | Tested | Built + clippy | Covered |
| macOS Accessibility / hotkey / clipboard backends | cfg-gated out | Compiled only; no AX at runtime | Needs local testing |
| `selara` CLI | Tested | Built | Covered |
| `selara` `serve` (egui picker) | cfg-gated out | Compiled only | Needs local macOS testing |
| `selara-desktop` (Tauri) | `dist/index.html` freshness only | App/DMG built and verified; release helper tests | GUI behavior needs local testing |

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
- Merging that pull request creates the tag and release. The CLI workflow includes the native Codex runtime and license notices. The reusable desktop workflow publishes verified app/DMG, Homebrew, and signed updater assets. Production requires complete Apple and updater credentials; it never silently falls back to ad hoc. `HOMEBREW_TAP_TOKEN` enables guarded, non-force tap publication. See the release guide for preflight and recovery details.
- For manual recovery from an old tag, preserve the tag's source and existing release assets, then run the current workflow from `main`: `gh workflow run desktop-release.yml --ref main -f tag=v0.4.0`. Check the run with `gh run list --workflow desktop-release.yml` and `gh run watch <run-id>`. Existing assets are reused; conflicts fail rather than being overwritten.
- All crates share the workspace version (`version.workspace = true`). Do not set a crate version by hand.
- Commits whose type is `build`, `ci`, `chore`, `test`, `style`, `meta`, or `license` do not appear in the changelog and do not trigger a release on their own.

### Desktop auto-update signing

The checked-in configuration enables updater artifacts and contains the persistent public key. Local builds override both to disable production updates; CI uses a separate temporary key. Production preflights Apple private-key access, notarization credentials, and updater signatures before compilation, then verifies all published artifacts. See [docs/release-updater.md](docs/release-updater.md) for secrets, recovery, backup/restore behavior, and the required manual upgrade from v0.4.1.

### Native Codex runtime

The app and CLI carry a native writing-only build of Codex 0.153.4. Build it with `scripts/codex-runtime/build.sh`; source revision, archive digest, patch digest, Rust version, and macOS minimum are pinned in `vendor/codex-runtime/runtime.toml`. Review and update the patch deliberately when upgrading Codex, then run the actual process and authentication-lock suites. See [vendor/codex-runtime/README.md](vendor/codex-runtime/README.md).

Settings account state is independent of unsaved provider settings. `provider.codex_home` selects an absolute shared Codex home before `CODEX_HOME`, then `~/.codex`. No separate Codex installation or Node runtime is needed. Remove legacy `CODEX_BIN` and `CODEX_AUTH_JSON` overrides; select the existing store directory rather than copying tokens. Selara's BYOK credentials are separate. Shared Codex sign-out removes credentials for the selected home across its supported official stores; other Codex processes can also observe that sign-out.

The managed CLI uses `selara serve --desktop-protocol`: bounded, versioned JSON lines on stdin/stdout, with logs on stderr. Status includes actual selection-process Accessibility trust and readiness. Quiesce acknowledges only after all workers finish and pending picker actions are cleared. External or incompatible processes have unknown trust and must be restarted under app management. Ordinary `selara serve` retains its existing logging behavior.

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
