/**
 * Markdown repair helpers applied before parsing — port of the desktop app's
 * `src/lib/markdown-repair.ts`.
 *
 * Resource folders may contain spaces (e.g. "William Stallings - Computer
 * Organization and Architecture D"), and OCR/model output frequently emits
 * the full path as a bare markdown image destination. CommonMark cuts such a
 * destination at the first space, so the image never renders. Wrapping it in
 * `<…>` keeps it whole; already-encoded or angle-bracketed destinations are
 * left alone.
 */

/** Wrap bare image destinations containing spaces in `<…>`. */
export function repairSpacedImageDestinations(markdown) {
  return String(markdown ?? '').replace(
    /!\[([^\]]*)\]\(([^)\n]+?\.(?:png|jpe?g|gif|webp|bmp))\)/gi,
    (whole, alt, destination) => {
      const trimmed = destination.trim();
      if (trimmed.startsWith('<') || !trimmed.includes(' ')) return whole;
      return `![${alt}](<${trimmed}>)`;
    },
  );
}

/**
 * Scan/model output sometimes escapes angle brackets — `\<table\>`,
 * `\<td\>` — which markdown then renders as literal text. Restore the
 * tags (with attributes and an optional closing slash) so real HTML
 * renders; the sanitizer still decides what is allowed.
 */
export function repairEscapedHtmlTags(markdown) {
  return String(markdown ?? '').replace(
    /\\<\s*(\/?)\s*([a-zA-Z][a-zA-Z0-9-]*)([^<>\\]*?)\s*\\>/g,
    '<$1$2$3>',
  );
}
