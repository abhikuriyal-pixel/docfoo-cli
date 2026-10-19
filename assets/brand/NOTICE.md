# GitHub artwork

`github-mark.svg` is the unchanged official `mark-github-24.svg` from GitHub’s
Primer Octicons repository. It appears before the GitHub label in both repository
buttons. A CSS mask makes the mark follow the button text color in either theme;
its geometry is unchanged.

Source: https://github.com/primer/octicons/blob/main/icons/mark-github-24.svg

SHA-256: `bce494189797623c34e39a41c2c38a132bdca23ce4e6ac06b70116ed7e91ce26`

Copyright (c) 2026 GitHub Inc. MIT license: `OCTICONS-LICENSE.txt`.
GitHub trademark rights remain with GitHub; inclusion does not imply endorsement.
The asset is served locally with no external runtime dependency.

# Hermes artwork

`hermes-girl.png` is an unchanged copy of the official Hermes Agent website
logo, `website/static/img/logo.png`, from Nous Research’s `hermes-agent`
repository at revision `d177b119e9c56c9ddc0b7379ffce52341ec06584`. It is the
black-and-white girl artwork, not the separate pixel-art Hermes mascot.
The same original image is used in both themes, with no recoloring or redraw.

Source: https://github.com/NousResearch/hermes-agent/blob/d177b119e9c56c9ddc0b7379ffce52341ec06584/website/static/img/logo.png

SHA-256: `2eaff911b9da9b1f1fcc81adb02f4992bb9ea6b781f4dd048cd79349927ddb7a`

Copyright (c) 2025 Nous Research. Repository MIT license: `HERMES-LICENSE.txt`.
Hermes names and artwork belong to their respective owners; inclusion does
not imply endorsement. The logo is served locally, with no runtime dependency
on the Hermes installation or its website.

# PiKins artwork

`pikins.svg` is an unchanged copy of PiKins’ official `build/icon.svg`, also used
as `assets/brand/icon.svg` on its landing page. The same fixed-color charcoal,
pale and sage artwork is used in both themes.

Source: https://github.com/abhikuriyal-pixel/PiKins/blob/main/build/icon.svg

SHA-256: `c8da71d9f095d8d9ae497805b21f5f6fe74cb23bcf4734a243a3397a10918712`

Copyright (c) 2026 abhikuriyal-pixel. MIT license: `PIKINS-LICENSE.txt`.
The public site serves its own copy; no sibling checkout is needed at runtime.

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
