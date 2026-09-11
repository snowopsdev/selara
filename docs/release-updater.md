# macOS releases and update recovery

Production releases require Developer ID signing, notarization, and a persistent
Tauri updater key. A public updater key is committed; private material belongs
only in secure storage and GitHub Actions secrets. Local builds stay usable
without these credentials. This source configuration does not mean a signed
production release has already been published.

## Build modes

Use `node scripts/release/build-desktop.mjs apps/selara-desktop` after `npm ci`.
Set `SELARA_BUILD_MODE` explicitly in automation:

| Mode | Apple signing | Updater |
| --- | --- | --- |
| `local` (default) | Ad hoc when absent; complete configured identities honored | Disabled, including the embedded key |
| `ci` | Ad hoc; no production credentials | Temporary test key, required and distinct from production |
| `release` | Developer ID and notarization required | Persistent production key required |
| `recovery` | Historical placeholder-key tags can retain ad hoc packaging; current tags require production credentials | Determined by the selected source tag |

Incomplete signing settings fail before compilation. Empty optional Apple
variables are removed; a configured certificate's password is preserved exactly,
including a valid empty password. A configured signing failure never falls back
to ad hoc. Production preflight signs a probe, checks notarization authentication,
and verifies the updater private key matches the embedded public key.

Required production secrets:

- `APPLE_CERTIFICATE`: base64 PKCS#12 containing a suitable Developer ID
  Application certificate **and its matching private key**.
- `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY` (full Developer ID name).
- `APPLE_ID`, `APPLE_PASSWORD` (app-specific password), `APPLE_TEAM_ID`.
- `TAURI_SIGNING_PRIVATE_KEY`, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`.

Never put these values in a PR, log, repository, or command transcript. Retain an
encrypted copy of the updater private key and a separately protected password;
test recovery from secure backup. Key loss or an incompatible public-key change
requires another manual installation. Rotate Apple certificates before expiry,
retaining a stable signing identifier and team. An active Apple Developer
membership is required; pending membership cannot sign or notarize releases.

## Publication and recovery

Release-please owns versions and tags. The release workflow publishes the
complete native CLI archive, then calls `.github/workflows/desktop-release.yml`.
The CLI and app include the pinned writing runtime, license, notices, and build
provenance. Tauri signs nested binaries and notarizes/staples the enclosing app.
After Tauri builds the signed DMG, the build helper submits that disk image to
Apple separately, requires an Accepted result, then staples and validates its
ticket before checksums or publication. Notarization waits up to 30 minutes;
a timeout or rejected submission stops publication. Release checks
verify the signing team, bundle signatures, stapled app/DMG tickets, Gatekeeper,
and DMG integrity. Only verified notarization removes the Homebrew caveat.

A production release has nine assets:

1. `selara-X.Y.Z-macos-arm64.tar.gz`
2. Its `.sha256`
3. `Selara-X.Y.Z-macos-arm64.dmg`
4. Its `.sha256`
5. `selara-cask.rb`
6. `selara-formula.rb`
7. `Selara-X.Y.Z-macos-arm64.app.tar.gz`
8. Its `.sig`
9. `latest.json` with a `darwin-aarch64` entry and a version-specific asset URL.

The manifest is uploaded last, after downloading and verifying its referenced
payload and signature. Publication is append-only and serialized per tag. CLI
reuse validates versions, required files, Developer ID signatures, and runtime
provenance before reusing or generating a checksum. Desktop recovery validates
release/tag history, matching application versions, and the published CLI pair.
It checks out app source from the selected tag and release tools from the workflow
commit. Updater-enabled recovery requires `APPLE_TEAM_ID` even when it skips the
build. Before updater signing, both the DMG app and the safely extracted updater
app must satisfy an Apple-anchored Developer ID requirement for that expected
team, including all three bundled executables. A valid signature or notarization
from another team is insufficient. Every recovered archive is also compared with
the verified published DMG, including archive-present/signature-missing cases.
Missing archives are derived from that exact app. macOS metadata sidecars are
excluded from archives. Conflicting assets fail without overwrites.

Run recovery from `main`, for example:

```sh
gh workflow run desktop-release.yml --ref main -f tag=v0.4.0
gh run list --workflow desktop-release.yml
gh run watch <run-id>
```

Historical releases keep their original six-asset inventory and disabled updater.
Optional tap publication requires `HOMEBREW_TAP_TOKEN`, verifies that the selected
tag is latest, rejects version downgrades, and never force-pushes. Recovering an
older tag never changes GitHub's latest release selection.

## Installation transaction

The app downloads with progress and verifies the Tauri signature before pausing
work or changing disk contents. Network requests have deadlines. A missing feed
is an error, not “up to date.” An OS lock keyed by the canonical app path excludes
concurrent installers. Selara creates and verifies a persistent sibling backup,
then asks the managed selection process to reject new work and finish existing
work. Only an acknowledged quiescence barrier permits stopping that process.

Archive extraction is bounded and rejects unexpected paths and escaping links.
The replacement is validated and moved on the app's volume using ordinary file
operations; no elevated installer is invoked. Handled replacement or validation
failures restore and verify the prior app before restarting its service. Failed
restoration retains the verified backup and reports its path. Unsupported
installation permissions offer the DMG instead. Backups do not guarantee recovery
from power loss or filesystem failure.

## First upgrade and release gate

v0.4.1 has no trusted updater key and cannot bootstrap this trust automatically.
Install the first corrected, notarized DMG manually. Later releases can update
in-app. Moving from an ad hoc executable to Developer ID may require one-time
Accessibility reapproval; restart the app-managed background service afterward.
An explicit Finder launch opens Settings. Login startup uses `--background` and
keeps Settings and the picker hidden. Existing enabled login items are refreshed
with the new arguments; disabled login items stay disabled. If refreshing fails,
Recent output reports it; switch Start at login off and on once to retry.

Before publishing, validate two distinct production-signed builds through an
isolated feed: actual account use, Finder launch, hidden picker, Accessibility
changes, successful update/relaunch, tampering, interrupted download, busy work,
concurrent installation, replacement failure, and failed restoration. Temporary
CI keys and ad hoc fixture transitions exercise implementation paths but do not
substitute for the final signed/notarized rollout check.
