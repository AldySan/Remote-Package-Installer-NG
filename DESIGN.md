# Design direction

This file records the UI direction for Remote Package Installer NG so future
UI work has an anchor instead of re-deriving taste from scratch.

**Direction:** modern minimalist, clean, professional.
**Brand soul:** comfort and ease. The user is sending large files to a game
console over their home network; the app's job is to make that feel boring
(in the best way) and obvious at every step.

## Dials (antislop Liveliness Toolkit)

- **ENERGY 1** — calm. No racing counters, no celebratory states. A finished
  transfer is a quiet green label.
- **RHYTHM 1** — steady. The layout does not reflow on its own; things move
  only when the user acts.
- **MOTION 1** — minimal. The only moving parts are spinners and progress
  bars, because they report real work. No decorative animation, no hover
  transitions, no gradients or glow. Anything that moves must be reporting
  engine state.

## Color

One semantic color per meaning, per theme, centralized in
`crates/rpi-ng/src/palette.rs`:

| meaning | signals |
|---|---|
| `ok` (green) | connected, completed, server OK, upload rate |
| `warn` (amber) | needs user attention: paused, unknown outcome, warnings |
| `error` (red) | failed/invalid items, errors, incompatible console |
| `info` (blue) | work in progress: connecting, preparing, transferring |
| `installing` (violet) | the console is installing the package |
| `neutral` (gray) | no signal: queued, cancelled, no console |

Rules:

- Every colored text must pass WCAG AA (>= 4.5:1) on its background in BOTH
  themes; the palette module's unit tests pin this. If a color change breaks
  that test, the color is wrong, not the test.
- Color is a signal, never decoration. If a piece of text does not map to one
  of the six meanings, it gets the normal text color.
- No hardcoded RGB values in views. Everything goes through the palette.

## Typography and layout

- egui default fonts and sizes; no custom fonts. The tool should feel native,
  not branded.
- Monospace only for machine identifiers: URLs, content IDs, log lines.
- Layout follows content need: the queue toolbar wraps into two rows below
  1050px of available width so it never collides at the minimum window size
  (860x560); the threshold is pinned by measured layout tests. The top bar
  does the same below 960px, and the console status chip truncates at 320px
  with the full text on hover, so a long backend identity or incompatibility
  reason can never collide with the right-aligned controls.
- Empty, loading and error states are real screens, not afterthoughts
  ("Drop PKG files here", spinners with a label, actionable error text).

## Copy

- Plain, short, specific. "Start queue", "Check File Server", not "Get
  Started".
- Every number shown comes from the engine. No illustrative or fake data.
- Errors say what to do next, not just what went wrong ("check the firewall
  allows this app on the local network").
- No em dashes in user-facing strings; use periods, colons, semicolons or
  parentheses instead.
