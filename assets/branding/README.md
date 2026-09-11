# Selara artwork

`quotation-mono/` preserves the user-supplied `selara-quotation-mono.zip`, without
macOS `._` metadata files. Its README, prompts, and previews describe the source
artwork; they are retained as provenance, with trailing blank lines normalized
in the two Markdown files.

Settings and the menu bar use the supplied transparent quotation mark directly:

- Settings: an exact copy of `template/48x48.png` at
  `apps/selara-desktop/assets/quotation-mark.png`, rendered at 24 CSS pixels as
  an alpha mask using the theme's text color. The local copy is accessible to
  Vite's development server; production builds embed it in `dist/index.html`.
- macOS menu bar: `template/36x36.png`, embedded in the Rust binary and rendered
  as a native template icon, so macOS controls its color.
- App bundle: `app-icon.png` adds an opaque off-white square behind the
  quotation artwork. The four bundle PNGs in
  `apps/selara-desktop/src-tauri/icons/` are 32px, 128px, 256px, and 512px exports
  named `32x32.png`, `128x128.png`, `128x128@2x.png`, and `icon.png`.

macOS application icons do not receive template tinting. The opaque background
keeps the black artwork visible against dark Finder and Dock backgrounds; only
the menu-bar and Settings sources stay transparent. The original package's
rounded tiles are layout mockups. The separate production master was edited
with built-in imagegen; [the prompts](app-icon-prompt.md) record its provenance.

To export the bundle sizes with the installed Tauri CLI, run from
`apps/selara-desktop`:

```sh
npx tauri icon ../../assets/branding/app-icon.png --output /tmp/selara-app-icons --png 32 --png 128 --png 256 --png 512
cp /tmp/selara-app-icons/32x32.png src-tauri/icons/32x32.png
cp /tmp/selara-app-icons/128x128.png src-tauri/icons/128x128.png
cp /tmp/selara-app-icons/256x256.png src-tauri/icons/128x128@2x.png
cp /tmp/selara-app-icons/512x512.png src-tauri/icons/icon.png
```

After changing the Settings artwork, run `npm run build` in
`apps/selara-desktop` and commit the regenerated `dist/index.html`.

## Integration verification (2026-09-11)

- `npm run build`: passed; both CSS masks embed the exact supplied 48px PNG.
- `npm test`: all 36 frontend tests passed.
- `cargo fmt --all --check`: passed.
- `cargo clippy -p selara-desktop --all-targets --locked`: passed on macOS.
- `cargo build -p selara-desktop --locked`: passed on macOS, with locally staged
  sidecars and runtime notices from the same source revision.
- All 17 image and preview files match the archive byte for byte; the two
  Markdown files only normalize trailing blank lines. All 14 template PNGs
  have the expected dimensions and RGBA format. The Settings copy matches its
  source template; the menu-bar template is unchanged.
- All four bundle PNGs have the expected dimensions, fully opaque alpha
  (`255` for every pixel), and a light background covering over 87% of each
  image. The former transparent bundle PNGs had no opaque light pixels.
- Playwright checked the built Settings page at 920 × 640 with mock native API
  responses in [light](../../docs/screenshots/branding/settings-light.png) and
  [dark](../../docs/screenshots/branding/settings-dark.png) appearances. The
  development server also served the 48px mark successfully at its local asset
  URL, displayed at 24 CSS pixels.

- `npx tauri bundle --debug --bundles app --ci --no-sign --config
  '{"bundle":{"createUpdaterArtifacts":false}}'`: passed using the rebuilt
  debug executable and the same local Cargo target directory. This produced
  a local test bundle; release signing was not exercised.
- The generated bundle's `CFBundleIconFile` points to `Selara.icns`.
  `iconutil -c iconset` decoded the packaged icon successfully; every decoded
  representation has fully opaque alpha.
- [Finder's native Quick Look](../../docs/screenshots/branding/finder-app-icon.png)
  displays the packaged app icon clearly against the dark preview background.

The native menu-bar rendering has not been visually checked in a running app.
The Settings browser screenshots do not verify native vibrancy.
