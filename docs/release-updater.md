# Release workflow and updater status

The normal macOS release path calls the reusable
`.github/workflows/desktop-release.yml` workflow. It runs
`node scripts/release/build-desktop.mjs apps/selara-desktop`, which invokes
the installed Tauri CLI with `build --ci --bundles app,dmg`, then verifies the
app signature, bundled CLI, and DMG before publishing release assets.

For manual recovery from an older tag, keep the old tag's source and existing
assets, and run the current workflow from `main`:

```sh
gh workflow run desktop-release.yml --ref main -f tag=v0.4.0
gh run list --workflow desktop-release.yml
gh run watch <run-id>
```

The recovery tooling reuses existing assets and fails on conflicts. It does
not edit version or tag metadata. The macOS build defaults to ad-hoc signing
with identity `-`; empty optional Apple credentials are omitted. A real
Developer ID build requires `APPLE_CERTIFICATE` and
`APPLE_SIGNING_IDENTITY`; `APPLE_CERTIFICATE_PASSWORD` may be empty. Notary
credentials must be either all absent/empty or all present:
`APPLE_ID`, `APPLE_PASSWORD`, and `APPLE_TEAM_ID`. They are accepted only
with real signing. The Homebrew caveat is removed only when
`xcrun stapler validate` succeeds; that check validates a stapling ticket and
does not replace the other release verification steps.

## Updater artifacts are not active

The checked-in `tauri.conf.json` contains the placeholder
`REPLACE_WITH_TAURI_UPDATER_PUBKEY` and sets
`bundle.createUpdaterArtifacts` to `false`. The app's updater runtime remains
unconfigured, and current releases do not publish `latest.json` or updater
tarballs. A placeholder or empty key is deliberately treated as disabled by
the build helper.

## Requirements for future activation

Enable updater publishing only after all of these are implemented and
verified together:

1. Generate a Tauri updater keypair outside the repository and commit the real
   public key in `plugins.updater.pubkey`.
2. Store the matching private key and password as
   `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`.
3. Enable `bundle.createUpdaterArtifacts` and make the release workflow fail
   if the private key is missing or the generated signature is absent.
4. Upload the signed updater tarball and its `.sig`, generate `latest.json`
   with matching version, URL, and signature, and verify the published assets
   against the committed public key.

Do not describe updater publishing as active until the workflow performs and
checks all of those steps. Never commit the private key or its password.
