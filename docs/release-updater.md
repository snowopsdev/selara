# Release workflow: updater artifacts

The desktop app (`apps/selara-desktop`) ships with `tauri-plugin-updater`. On
launch (after 10 s) and every 24 h it fetches

```
https://github.com/snowopsdev/selara/releases/latest/download/latest.json
```

and offers to install any newer version whose signature checks out against
`plugins.updater.pubkey` in `tauri.conf.json`. For that to work, the
`desktop` job in `.github/workflows/release.yml` (added by the DMG/Homebrew
release PR, #70) needs two additions. They are not in the workflow yet
because the branch this doc lands on predates that job; apply them once the
job exists.

Prerequisites, done once by a maintainer (details in `CONTRIBUTING.md`,
"Desktop auto-update signing"):

- `npx tauri signer generate -w ~/.tauri/selara.key`, public key committed in
  `tauri.conf.json` (replacing `REPLACE_WITH_TAURI_UPDATER_PUBKEY`).
- Repository secrets `TAURI_SIGNING_PRIVATE_KEY` (file contents) and
  `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`.

`bundle.createUpdaterArtifacts` is already `true`, so `tauri build` writes
`target/release/bundle/macos/Selara.app.tar.gz` and
`Selara.app.tar.gz.sig` next to the `.app` whenever the signing key is in the
environment. Without the key the build still succeeds but no `.sig` is
produced, and the steps below skip the upload.

## Step 1: pass the signing key to `tauri build`

Add the two secrets to the `env:` of the existing "Build app and DMG" step.
`--bundles app,dmg` is enough; the updater tarball is produced alongside the
`app` bundle.

```yaml
      - name: Build app and DMG
        working-directory: apps/selara-desktop
        env:
          APPLE_CERTIFICATE: ${{ secrets.APPLE_CERTIFICATE }}
          APPLE_CERTIFICATE_PASSWORD: ${{ secrets.APPLE_CERTIFICATE_PASSWORD }}
          APPLE_SIGNING_IDENTITY: ${{ secrets.APPLE_SIGNING_IDENTITY }}
          APPLE_ID: ${{ secrets.APPLE_ID }}
          APPLE_PASSWORD: ${{ secrets.APPLE_PASSWORD }}
          APPLE_TEAM_ID: ${{ secrets.APPLE_TEAM_ID }}
          # Updater artifact signing (tauri-plugin-updater). Optional: without
          # these the DMG still builds, but no updater tarball is published.
          TAURI_SIGNING_PRIVATE_KEY: ${{ secrets.TAURI_SIGNING_PRIVATE_KEY }}
          TAURI_SIGNING_PRIVATE_KEY_PASSWORD: ${{ secrets.TAURI_SIGNING_PRIVATE_KEY_PASSWORD }}
        run: npx tauri build --bundles app,dmg
```

## Step 2: publish the updater tarball, its signature, and `latest.json`

Insert after "Upload DMG and Homebrew files to the release". The manifest
format is the one `tauri-plugin-updater` reads (`version`, `notes`,
`pub_date`, `platforms.<target>.{signature,url}`). The GitHub-hosted runner
is Apple Silicon, so the target key is `darwin-aarch64`; add a
`darwin-x86_64` entry from an Intel build if one is ever published.

```yaml
      # Updater feed for tauri-plugin-updater. The app fetches
      # releases/latest/download/latest.json and verifies the .sig against the
      # public key in tauri.conf.json. Skipped when the signing key is unset.
      - name: Upload updater artifacts and latest.json
        if: ${{ env.TAURI_SIGNING_PRIVATE_KEY != '' }}
        env:
          GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}
          TAURI_SIGNING_PRIVATE_KEY: ${{ secrets.TAURI_SIGNING_PRIVATE_KEY }}
          VERSION: ${{ needs.release-please.outputs.version }}
          TAG: ${{ needs.release-please.outputs.tag_name }}
        run: |
          set -euo pipefail
          TARBALL=$(ls target/release/bundle/macos/*.app.tar.gz | head -n 1)
          SIG="${TARBALL}.sig"
          test -f "$SIG" || { echo "missing updater signature ${SIG}"; exit 1; }
          ASSET="Selara-${VERSION}-macos-aarch64.app.tar.gz"
          mkdir -p dist/updater
          cp "$TARBALL" "dist/updater/${ASSET}"
          cp "$SIG" "dist/updater/${ASSET}.sig"
          NOTES=$(gh release view "$TAG" --json body --jq .body)
          jq -n \
            --arg version "$VERSION" \
            --arg notes "$NOTES" \
            --arg pub_date "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
            --arg signature "$(cat "$SIG")" \
            --arg url "https://github.com/${GITHUB_REPOSITORY}/releases/download/${TAG}/${ASSET}" \
            '{version: $version, notes: $notes, pub_date: $pub_date,
              platforms: {"darwin-aarch64": {signature: $signature, url: $url}}}' \
            > dist/updater/latest.json
          gh release upload "$TAG" \
            "dist/updater/${ASSET}" "dist/updater/${ASSET}.sig" dist/updater/latest.json --clobber
```

## Checking a release by hand

```sh
curl -sL https://github.com/snowopsdev/selara/releases/latest/download/latest.json | jq .
```

should show the new version and a `darwin-aarch64` entry whose `url` resolves
(HTTP 302 then 200). Then open Selara → Status → Updates → "Check for updates"
on an older build; the card should offer "Install and restart".
