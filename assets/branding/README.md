# Selara artwork

`quotation-mono/` preserves the user-supplied `selara-quotation-mono.zip`, without
macOS `._` metadata files. Its README, prompts, and previews describe the source
artwork; they are retained as provenance, with trailing blank lines normalized
in the two Markdown files.

The app uses the supplied transparent quotation mark directly:

- Settings: an exact copy of `template/48x48.png` at
  `apps/selara-desktop/assets/quotation-mark.png`, rendered at 24 CSS pixels as
  an alpha mask using the theme's text color. The local copy is accessible to
  Vite's development server; production builds embed it in `dist/index.html`.
- macOS menu bar: `template/36x36.png`, embedded in the Rust binary and rendered
  as a native template icon, so macOS controls its color.
- App bundle: exact copies of `template/32x32.png`, `128x128.png`, `256x256.png`,
  and `512x512.png` in `apps/selara-desktop/src-tauri/icons/`, respectively named
  `32x32.png`, `128x128.png`, `128x128@2x.png`, and `icon.png`.

The package has no finished app-tile image. The bundle uses the bare supplied
mark; the light and dark rounded tiles in the source preview are layout mockups.
If the bundle artwork changes later, keep the transparent menu-bar source
separate so native template tinting continues to work.

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
  have the expected dimensions and RGBA format. The five runtime PNG copies
  match their source templates.
- Playwright checked the built Settings page at 920 × 640 with mock native API
  responses in [light](../../docs/screenshots/branding/settings-light.png) and
  [dark](../../docs/screenshots/branding/settings-dark.png) appearances. The
  development server also served the 48px mark successfully at its local asset
  URL, displayed at 24 CSS pixels.

The native menu-bar rendering and packaged Finder icon have not been visually
checked in a running app. Browser screenshots do not verify native vibrancy.
