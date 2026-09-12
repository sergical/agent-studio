# Skill row — decided from the `PROTO:skill-row` picker (2026-09-12)

The Skills list row and the Home inbox row share one design. It was chosen from an in-place
prototype picker over eleven rounds; the picker and its variants are deleted.

## Direction: the Stack row

- Columns: glyph hit area · Skill name · Location · Harnesses · Prompt / Full tokens. Rows are
  36px (`h-9`). No Description column.
- The leading glyph is the action: the row's one state icon (update, trial, broken, drift,
  invocation) with a menu on hover. Colour and icons carry state; words do not.
- Location shows the Global chip and up to two project chips, then `+N`.
- Harnesses is an overlapping avatar-group stack: the Universal folder disc first when present,
  then one disc per reached harness up to five, then a `+N` disc.
- Harness disc badges: a small link glyph when the harness reaches the skill only through the
  Universal folder; a red broken-link glyph when that link is broken.
- Disabled treatment: **Muted**. A disabled or parked disc keeps its shape and dims the glyph to
  `--color-icon-muted` at `opacity-60`. Parked marks every disc and dims the whole row; disabled
  marks one disc.

## Tooltips

Every harness disc has a one-line tooltip because an 11px icon cannot show how it reaches the
skill: `<glyph> <harness> · <how>[ · <state>]`.

- how: `linked to Universal folder` / `source in Universal folder` / `own copy`; a broken link
  replaces it with `link broken` in the error tone.
- state: `disabled` or `parked` in the warning tone, appended after the how-word.
- The Universal disc says `Universal folder`. The `+N` disc lists the hidden harnesses.
- Location: `+N` lists the hidden projects; a project chip has a tooltip with its full name
  only when it is truncated. The Global chip never has one.
- The invocation glyph says `You run it` or `The model runs it`. Nothing else in the row has a
  tooltip.

## Rejected

- Per-harness invocation tables and frontmatter-key captions in tooltips: a dictionary, not a
  row.
- A Description column: the name column needs the width more.
- Native `title` attributes: inconsistent look and delay.
- Six- and ten-harness toggles, "linked by" lists on the Universal disc, verbose disabled
  reasons, "N copies", "own copy" next to the Global chip: restate what the row already shows.
- Path tooltips (mono path only) and Map tooltips (whole-stack grid): "people know a global
  skill is at the home directory"; they spell out the obvious.
- Exception-only tooltips (nothing on a healthy disc): went too far, the icons are too small to
  tell symlinked from own copy.
- Gallery caption headers: the list must look like the end product.
- Disabled treatments Slash (thin diagonal line) and Dashed (dashed outline): the line alone
  did not read as disabled once the glyph stayed full tone; Muted is enough on its own.
- Earlier row variants Ghost, Reach, Bare, Lead: replaced by the Stack row in earlier rounds.

## Open

- Read-only copies (plugin cache, read-only own copy) have no mark in the row; the detail view
  carries that.
