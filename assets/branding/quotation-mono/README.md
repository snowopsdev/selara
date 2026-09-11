# Selara — monochrome quotation identity

The selected Quotation design, rendered as a single-color transparent mark. Two opposing quotation shapes preserve the diagonal composition and open central gap.

## Assets

- [Template master](mark-template-master.png): original generated black mark with alpha.
- [Template PNGs](template/): 16, 18, 20, 22, 24, 32, 36, 40, 44, 48, 64, 128, 256, and 512 pixels.
- [Visual preview](preview.html): light and dark appearances, small sizes, and app-tile layout mockups.
- [Preview image](preview.png): saved browser screenshot of that comparison.
- [Generation prompts](PROMPTS.md): built-in imagegen provenance and exact prompts.

## Small system icons

Use the bare mark for the menu bar and compact controls. Use 18×18 or 20×20 at 1×, paired with 36×36 or 40×40 at 2×. The 22×22 and 44×44 files provide a larger option. Each file includes transparent padding.

The black PNG is a template source: native template rendering should tint the alpha for the surrounding menu-bar appearance. The preview shows the same alpha in black and white; it does not introduce a second, potentially different silhouette. In a web view, the mark may be used as a CSS mask with the current text color.

## Verification

All 14 template PNGs have the expected dimensions, an alpha channel, and fully transparent corners. A 4-connected pixel analysis at alpha ≥ 0.5 found exactly two components in every size, including 16 pixels. The two quotation shapes therefore remain separate at the tested raster sizes.

The browser preview was visually inspected on light and dark backgrounds, including menu-bar examples. These checks concern the assets; the running Selara application has not been changed or tested with them.

## App icon treatment

The preview includes matching light and dark app-tile mockups composed in HTML from the exact template PNG. These demonstrate layout and monochrome treatment. A finished native app-bundle icon is not included in this pack.

Separate generated app-tile experiments painted checkerboard pixels into the outside margin. Those outputs were rejected as production assets; only the transparent standalone mark is included.
