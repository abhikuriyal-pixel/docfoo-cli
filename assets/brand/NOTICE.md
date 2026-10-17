# DocFoo artwork

Landing-page favicon: `docfoo-white.svg` uses the adopted Genie geometry below,
with white fills for visibility in the browser tab. `docfoo.svg` remains the
unchanged source artwork used by the page footer. The hero SVG uses the same
source paths, separating the two fog subpaths for subtle CSS motion. Its face
geometry and expression remain static. Reduced-motion preferences disable
animation; off-screen and hidden-tab animation is paused.

`docfoo-animated.svg` and `scan-animated-face.svg` reuse the corresponding
artwork in the sibling DocFoo project's `src/assets/masks/` directory, as
requested by the user. The animated heading scenes retain their flat masks;
the owl avatar uses the approved monochrome pixel Genie logo.

The geometry and animation timing are preserved. Embedded SVG styles were
moved into `animations.css` for PiKins' Content Security Policy, with
scoped selectors and prefixed keyframe names to avoid affecting other avatars.
Colors inherit the PiKins theme; reduced motion keeps a static scene.
No external artwork or sibling-project files are needed at runtime.

## Laughing Fold concept

`docfoo-laughing-fold.svg` is a new, single-color product-mark concept inspired
by the carved laughing-mask identity in `DocFoo/assets/icon.svg`. Its dipped
brow, crescent cutouts, grin, and rising hook are redrawn as one solid silhouette
with transparent facial cutouts, without the original grain or flower props.
`docfoo-laughing-fold-pixel.svg` is a hand-tuned 16px interpretation for small
surfaces. Both inherit `currentColor` and have transparent backgrounds.

The plain-mask assets are retained as original concept references. After
`npm run build`, run `npx electron scripts/preview-docfoo-logo.mjs` to export
the current Genie artwork and live pixel-owl preview in `docs/design/`.
Use `--original` to view the earlier plain-mask concept instead.

## Genie revision

`docfoo-laughing-fold-genie.svg` preserves the entire original mask, expression,
and upper-right hook. A tapered wisp flows behind the mask and curls beneath
the chin, using the same `currentColor` as the mask. It is inspired by the
user's genie reference, not traced or pixel-matched; the standalone product
mark uses native SVG curves, with no stars or extra props. The background and
facial cutouts remain transparent.

`docfoo-laughing-fold-genie-pixel.svg` retains the original pixel-mask geometry
on a 20px grid, adding stepped wisps around the lower face and a hollow curled
tail below the chin. The face, upper-right hook, and wisps share one color.

This is the adopted DocFoo identity in PiKins. `pixel-docfoo.ts` renders
the same approved 20px pixel Genie in every shared owl avatar; tests enforce
pixel-for-pixel parity with the SVG asset. The mask, hook and curl are inset
within the screen without changing the owl's body, colors or motion. The
live avatars never embed the smooth product SVG.
