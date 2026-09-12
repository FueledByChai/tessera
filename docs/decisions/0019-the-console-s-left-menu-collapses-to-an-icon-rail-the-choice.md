# 0019 — The console's left menu collapses to an icon rail; the choice is per-browser

Status: accepted
Date: 2026-09-11

## Context

The sidebar (`web/app/page.tsx`, `<aside className="sidebar">`) has been a fixed column since
the console's first slice — 224 px, 216 px in terminal mode — holding the brand, ten nav items
with a glyph and a label each, and the engine-status foot. The dense nine-column tables of
UI-03 to UI-06 (the strategies catalog, the run history, the studies heat map) pay for that
width on a 1280 px laptop, and the owner asked for the menu to collapse to icons (grill-me,
2026-09-11, story BT-1103). The console's only other chrome preference, the display mode, is
already per-browser (`bt-display-mode` in `window.localStorage`), and nothing in the catalog
holds a display setting. Three shapes were on the table for the collapse itself — a toggle, a
hover-expand rail, or both — and two places for the preference to live.

## Decision

The left menu collapses behind a control in the sidebar to a 56 px icon rail — the same width
in both display modes — keeping the BT brand mark, the ten nav icons, and the engine-status
dot, with the labels and the foot's text not rendered and each label shown as a hover tooltip;
the active item keeps its amber colour and inset bar. It is never automatic: no viewport width
collapses it. The collapsed choice is a per-browser preference in `window.localStorage` under
`bt-sidebar` (values `collapsed` and `expanded`), never a catalog or server setting; a fresh
browser starts expanded. The preference store for navigation chrome is therefore browser
storage, beside the display mode.

## Alternatives

- No collapse; let the wide tables scroll: no new control or stored state, but a 1280 px
  laptop keeps losing the width the dense grids need.
- A hover-expand rail with no control and no stored state: nothing to click, but it moves
  under the pointer while the owner is aiming at the content beside it, and it cannot be
  pinned collapsed.
- A toggle plus a hover preview: the two behaviors fight — the rail widens on the way to the
  content the collapse was meant to reveal.
- Store the preference in the local catalog or on the service: it would follow the owner to
  another browser, but it is a per-device display preference, not research state, and the
  catalog is the record of runs; the display mode already set the opposite precedent.

## Consequences

`web/app/globals.css` gains rail rules under both themes (`.sidebar` collapsed, the tooltip),
`web/app/page.tsx` gains a `collapsed` state and a toggle in the brand row, and the layout
check gains a collapsed pass at 1280 and 1440 px in both display modes. `bt-sidebar` joins
`bt-display-mode` as browser storage, so the state is per browser and per device and is not
backed up with the catalog. The nav glyphs now have to read alone: the `</>` icon is three
characters and must stay inside the rail's icon column, and a future icon that is not legible
without its label needs a different glyph. The Code page's navigator and the catalog inspector
keep their current behavior; if they later collapse too, this record is the precedent for the
shape and the store.

## What would show this was wrong

The owner keeps the rail collapsed but still reaches for the labels by another route (the
glyphs are not learnable alone), or asks for the collapsed state to follow them to another
browser or machine. Then the glyphs get permanent text or the preference moves to the catalog,
in a superseding record.
