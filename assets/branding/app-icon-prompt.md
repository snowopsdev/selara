# Application icon background

Mode: built-in imagegen, editing the supplied quotation artwork. The selected
output is `app-icon.png`; the original `quotation-mono/` package is unchanged.
Tauri's icon CLI produces the bundle PNG sizes from this master.

The first edit produced a checkerboard in its outer margin. It was rejected
as a production asset. The background correction produces an opaque square
with no transparent margins, so the mark stays visible on dark backgrounds.

## Initial edit

Use case: precise-object-edit. Asset type: production macOS application bundle icon for Selara. Image 1 is the edit target: the supplied transparent black quotation mark master. Image 2 is supporting reference showing the same symbol visibly and the intended light app tile at bottom left; do not reproduce the reference sheet. Make ONE square 1024x1024 production icon. Preserve the exact two black opposing quotation silhouettes, their diagonal upper-left/lower-right arrangement, orientation, proportions, and open central gap from image 1. Add an opaque solid neutral off-white (#fafafa) rounded-square application tile underneath the mark, matching the light tile mockup in image 2. Tile should occupy about 86 percent of the full canvas width and height with clean smoothly rounded corners and equal outer margins. Center the mark on the tile, with comfortable even inset; the full pair should occupy about 64 percent of the full canvas width and height. The entire tile including the gap between quotation shapes MUST be fully opaque so the black artwork stays visible on a dark Finder background. Only the area OUTSIDE the rounded tile is genuinely transparent alpha. Flat monochrome treatment: uniform black mark, uniform off-white tile, crisp antialiased edges. No redraw or reinterpretation of the logo, no added shapes, no gradients, no lighting, no bevels, no shadows, no texture, no checkerboard pixels, no text, no labels, no screenshot/mockup or multiple icons. This is a background addition for dark-background legibility, not a new brand design.

## Final background correction

Use case: precise-object-edit. Edit this application icon to fix the background only. Keep the exact two black quotation shapes and their current size, positions and contours. Replace EVERY pixel outside the black quotation mark with a completely uniform opaque off-white #fafafa background, including the checkerboard, the rounded-square perimeter, and all outer margins. The final image is a simple black quotation mark centered on a SOLID OPAQUE OFF-WHITE SQUARE all the way to all four edges. NO transparency, NO checkerboard, NO rounded-square boundary, NO shadows, NO texture, NO gradient, NO border. The entire square canvas must be opaque. The only visible content is the same black symbol on a flat solid off-white square. This is production app artwork to stay legible on dark desktop backgrounds.
