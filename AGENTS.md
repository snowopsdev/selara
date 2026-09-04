# Agent Instructions

Rust workspace: `crates/selara-core` (config, commands, providers), `apps/selara` (CLI + macOS `serve`), `apps/selara-desktop` (Tauri Settings UI, single file `index.html`). Setup and platform notes: `CONTRIBUTING.md`.

## Commands
| Task | Command |
|------|---------|
| Format | `cargo fmt --all` |
| Lint | `cargo clippy -p <crate> --all-targets` |
| Test one crate | `cargo test -p selara-core` |
| Test workspace | `cargo test --workspace` |
| Desktop dev (hot reload) | `cd apps/selara-desktop && npx tauri dev` |
| Desktop dist rebuild | `cd apps/selara-desktop && npm run build` |

## Pull Requests
Every PR description uses exactly these `##` sections, in this order, each filled with real context (CI fails otherwise, see `.github/workflows/pr-template.yml`):

```markdown
## What Problem This Solves
## Why This Change Was Made
## User Impact
## Developer Impact
## Evidence
```

- Template with per-section guidance: `.github/PULL_REQUEST_TEMPLATE.md`. Use it as the body; do not substitute another format.
- Evidence includes commands run with results, test output, and screenshots for any UI change.
- This applies to any tool or agent opening or editing a PR, including `gh pr create --body`.

## Commits
- Conventional format: `type(scope): Subject` (`feat`, `fix`, `ref`, `docs`, `build`, `ci`, `chore`).
- Commit types drive semantic versioning via release-please (`feat` minor, `fix` patch, `!` breaking). Never edit version numbers by hand; see `CONTRIBUTING.md`.
- AI commits MUST include:
```
Co-Authored-By: (the agent's name and attribution byline)
```

## Conventions
- Never commit API keys, tokens, `.env`, or personal `config.toml`.
- `apps/selara-desktop/index.html` is the whole Settings UI; `dist/` is generated, rebuild it when the UI changes.
- Existing element ids and `invoke` command names in the UI are load-bearing; keep them when restyling.
- macOS-only paths (Accessibility, hotkeys, Tauri) are not covered by Linux CI; verify locally and say so in Evidence.
