# SSH Control Center — Design System "Relic Night" (v6)

A night sky over the fleet, as the Stardust Halo prototype: deep blue to violet, an ember haze where the halo sits, scattered stars, film grain.
The fleet is a halo of stardust around this Master, one bright spark per host; a spark swells when its host changes. The rainbow lives only where you point.
Dreamy where it is sky, precise where it is data.

Supersedes "Engraved Violet" (`DESIGN-v5.md`, kept for reference). The prototype that settled every value below is
`B16 · Relic Night` (local `.oil-ui/motion-directions/explore/b16-night.html`, not in git).

## Golden rules (do not break)

1. **One black ground, one sky.** The window is a single near-black gradient. The sky (top of Inventory) and the ground continue
   under the sidebar with no panel, border or seam. Nothing may stop at the sidebar's edge.
2. **Soft lives in the sky, hard lives in the data.** Sky, haze, grain, light leak, the halo's glitter, italic "online." are soft.
   Film edge, callout block, numerals, tabs, rows, host names are crisp: hairlines, square or 6 px corners, heavy weights.
3. **The rainbow is a response, not a decoration.** The iridescent pearl appears on hover, focus and selection (and on a pointed pod cap).
   Everything at rest is black, silver, steel blue and the sky's own colour.
4. **Steel blue is the accent** (`--steel`). Status colours are status only. No other accent.
5. **Motion has a reason and stops.** It reacts to the fleet (a state change, a recovery) or to the pointer. Nothing loops except the
   breathing online LED in the rows, all opacity/transform. The sky itself is still at rest (clean, as the prototype). Idle budget: **< 1 % of one core**.
6. **Readable at a glance, all day.** Text on the sky and on the ground stays ≥ 7:1 (`--white`, `--lilac`). Muted text is a colour
   token, never opacity. Focus: `outline: 2px solid var(--steel); outline-offset: 3px`. Targets ≥ 44 px. Everything that moves stops
   under `prefers-reduced-motion`.
7. **Unisex, slightly masculine.** Few pinks at rest, tight radii (6 px; pills only for small chips), bold data type.

## Tokens

```css
:root {
  /* ground */
  --ground-0: #07080E;  --ground-1: #0B0C18;  --ground-2: #0E0F1E;   /* window gradient top → bottom */
  --ridge:    #0A0B17;                                               /* near ridge = ground at that height */
  --deep:     #05060E;                                               /* blocks that sit on the sky */
  /* text */
  --white: #EEF0F8;   /* primary text */
  --lilac: #A9AEC8;   /* muted text, labels (≥ 7:1 on --ground-1) */
  --silver: #DCE0F0;  /* chrome highlights */
  /* accent + status */
  --steel: #8FB0F0;
  --led:   #7CF2C0;   /* online */
  --amber: #FFC266;   /* high load */
  --rose:  #FF7FAE;   /* offline */
  /* lines */
  --rule:  rgba(238,240,248,.12);
  --tint:  rgba(143,176,240,.07);
  /* the rainbow */
  --pearl: linear-gradient(100deg, #BFE4FF 0%, #E4CCF6 32%, #F6D6E4 52%, #CDF2DC 74%, #F4E6C6 100%);
  --pearl-faint: linear-gradient(100deg, rgba(191,228,255,.17) 0%, rgba(228,204,246,.17) 32%, rgba(246,214,228,.14) 52%, rgba(205,242,220,.14) 74%, rgba(244,230,198,.17) 100%);
  --ink-on-pearl: #0B0C18;
  --holo: conic-gradient(from 210deg, #9CFF57, #7CF6FF, #FFFFFF, #FF9CF2, #C3A6FF, #9CFF57);
  --chrome: conic-gradient(from 210deg, #F4F6FB, #A6B4D8, #E8ECF6, #6E7CA8, #DCD6EE, #C9E6EA, #F4F6FB);
}
```

### Sky clock

Since 2026-10-28 the Inventory sky is the prototype's fixed night; the clock below still colours the thin sky strip on the other pages.

The sky's four stops (`--sky0` zenith → `--sky3` horizon), the glow colour and the far-ridge colour are interpolated between hour
keyframes (smoothstep) and written to `<body>` **once a minute** from the viewer's local time. The sky and the sidebar both read them.

| From | Phase | sky0 → sky3 | Glow |
|---|---|---|---|
| 00:00 | night | `#04050C #0A0C1E #151A3C #26305E` | `#6C7CFF` |
| 04:18 | pre-dawn | `#04050D #0C0D24 #2A2052 #6E3E7E` | `#C98AD8` |
| 06:12 | sunrise | `#05071A #141A42 #4C3274 #E2805C` | `#FF9A5C` |
| 08:30–13:00 | day | `#050B22 #0E2052 #1E4494 #4E8AD6` | `#9AD0FF` |
| 16:18 | late | `#060A20 #121E4E #2A3A8A #6C72CC` | `#B0A8FF` |
| 18:06 | golden | `#06061A #1A183A #5C3252 #E28C4C` | `#FF8A40` |
| 19:24 | dusk | `#050514 #151232 #3C215A #8C3E7A` | `#C86AC8` |
| 21:00 | night | as 00:00 | |

- Sun: rises 06:00 behind this Master (33 % across the sky), sets 18:36 on the right; a white bloom; faint rays by day.
- Moon: a light crescent on its own arc 19:00 → 05:30.
- Stars: never below 40 % (visible all day), full at night. Dimmer under the sidebar text.
- Fleet health sets the glow's strength (`0.25 + 0.75 × online share³`, 3 s crossfade); offline hosts add a rose-blue chill at the horizon.

## Typography

All bundled (Tauri CSP: no font downloads). All OFL.

| Role | Face | Use |
|---|---|---|
| Headline numeral | Fraunces, `SOFT 0`, `opsz 144`, wght 900, 136 px | "9" in "9 of 11 online." |
| Landing word | Cormorant Garamond italic 400, 64 px | "online." — the one soft italic |
| Callout name, logo "SSH" | Fraunces `SOFT 0`, 500–800 | |
| UI: host names, menu, tabs, buttons | Instrument Sans 600–700 (Space Grotesk for menu/status caps, letter-spacing .14–.18em) | |
| Data: IPs, facts, film edge, labels | JetBrains Mono / Space Mono 11–13 px | |

Cormorant ships old-style figures only; its digits come from Fraunces (`unicode-range: U+0030-0039` face).
Thai: Noto Serif Thai (display, headings, numerals' words) + Noto Sans Thai Looped (body, UI, labels), chosen 2026-10-27 from four pairings compared in the real app. Variable fonts, Thai-only subsets (55 KB + 53 KB).

## Layout (main screen)

1. **Sidebar** — 236 px, transparent over the window ground. The sky continues under it down to the hill line (`--hill-y`, written by
   sky.js where the hill meets the plate's left edge), and the hill carries on under it in the same colours. Top → bottom, as the
   prototype: stacked logo (SSH / Control / CENTER); **Add host**, always in sight, the one solid button (white, pearl on hover);
   the menu (each row a faint pill; the active one pearl). Inventory's dropdown opens right under it — the filter and the host tree
   with curved connectors — folded by default, opened by its chevron and remembered; then Masters, Alarms, Service; current Master +
   round-cornered Refresh at the foot. Its sky carries the plate's stars,
   haze and grain (positions from sky.js), so nothing steps at the edge.
   No scrim over the sidebar's sky: its tree and foot words carry a dark halo instead.
2. **Sky plate** — the full window height (560–980 px), top of Inventory; the rows start below it. As the prototype: a fixed night
   (`#05071A → #16204A → #2A3566 → #0B0D1A`, ember haze `rgba(240,160,96,.22)` at the halo), scattered stars drawn once, a vignette,
   film grain (.32, overlay), the film edge (30 px, real readouts: online/total, Master, IP, last poll, offline), one hill, the
   **Stardust Halo** (a ring of glitter round this Master, a silver relic standing on the hill's crest;
   one spark per host, merged Masters larger), headline bottom-left, callout on the pointed spark. Drag turns and tilts the halo
   (inertia; double-click resets); a click opens the host; a mono hint bottom-right says so. The plate's bottom 30 px fade into the ground.
3. **Below the sky** — the summary line, then square tabs (All · Direct · Through <Master> per merged Master · Needs a look, each with a count)
   over host rows in two columns on hairlines (one column under 1000 px). "All" starts with this Master. The sky draws the topology; the rows are where you act.
4. **Every other page** (host, Master, Masters, Alarms, Service, Add host) — no sky plate; a faint 150 px strip of the same sky across the top,
   continuous under the sidebar. The sky clock runs on every page.
5. **Host page, Summary tab** — the facts first (address, routes, last status, service), then tags and the host's alarms, then the
   details you open on purpose (technical, edit, remove).

## Components

| Component | Rest | Hover / focus | Selected |
|---|---|---|---|
| Host row | hairline below, white name, lilac IP; state word in its status colour (online `--led`, offline `--rose`, high load `--amber`); not known (unknown, checking, no service) is a lilac word and a dashed ring | `--pearl-faint`, text stays light | `--pearl-faint` + 3 px pearl left edge |
| Fact (host page) | mono caps label, value in Instrument Sans 600 20 px (addresses in mono), sub-line lilac; last status adds response time and online for | — | — |
| Host key | hairline chip in the meta row: lock + "Host key pinned · TYPE", mono 11 px | — | — |
| Alarm level | dot + mono caps word in its colour (red `--rose`, yellow `--amber`) | — | — |
| Quiet row action (Acknowledge, Check now) | text only, lilac | hairline on row hover; `--pearl` on its own hover | — |
| Tab | square, 1.5 px rule border | `--pearl`, dark ink | `--pearl`, dark ink |
| Menu row | faint pill `rgba(238,240,248,.035)` | `--pearl` | `--pearl` + dark dot |
| Button (primary) | 6 px, hairline white | `--pearl`, dark ink | — |
| Callout | frosted ink block (`rgba(10,11,24,.72)` + 10 px backdrop blur), square, inner hairline, straight elbow leader, square terminal | — | — |
| Halo | 5 200 glitter points (ember `#F0A060`, cream, pale blue) in a tilted ring; far half behind the hill and relic, near half and a ¼-size bloom in front | — | — |
| Spark (host) | a flare in its status colour (cream online, rose offline, amber high load); offline and high-load sparks are named in mono | ×1.8 and named | callout on it |
| Planet (Master) | a merged Master (radius 11 % of the halo) or a Master below one (7 %): a lit silver-violet sphere with faint bands, a tilted cream-and-ember ring (far arc behind, near arc in front), a rim and glow in its status colour; spread round the halo, named in bold mono (on its left near the edge); this Master stays the relic | ×1.18; a pointer anywhere on it picks it | callout on it |
| Status dot | 9 px round in its colour (7 px in the tree) | | |

## Motion (all event-driven, all transform/opacity)

- State change: the host's spark swells and rings once (1.4 s), the row flashes once.
- Arrival: the first time the sky shows, the glitter gathers from scattered dust into the halo (1.8 s); a click skips it.
- Drag: the halo turns and tilts with inertia (pitch .10–.34); it draws only while it moves.
- Recovery: one shooting star across the upper sky.
- Count change: the numeral flexes (weight/axis + skew, 0.9 s).
- Pointer: parallax over the whole window — stars 4 px, halo, hill and callout 13 px, eased (900 ms), one rAF per move. The sidebar's
  copy of the sky moves with it (its stars by background-position, its hill by `translate`, the hill line at the curve's own slope at
  the seam), so nothing freezes at the sidebar's edge. The callout glides (560 ms).
- Always on (cheap): the online LED breathing in the host rows. Nothing in the sky loops.
- Nothing runs while the window is hidden. Nothing moves under `prefers-reduced-motion`.

## Implementation notes

- Tauri CSP `style-src 'self'`: no `style=""` attributes. Set custom properties through CSSOM (`el.style.setProperty`).
- Grain and the light leak use `data:` SVG backgrounds in the prototype — check `img-src` before shipping, or ship them as files.
- `backdrop-filter` only on a few static blocks (callout, film edge, preview controls).
- The halo is canvas 2D with hand-projected points (no library, no WebGL), so it draws the same in WebKitGTK without a GPU.
  At rest it draws nothing (measured 2026-10-28: 0 frames, sky page 0.12 % of a core).
- **No `preserve-3d` / `translateZ` anywhere.** WebKitGTK (the Ubuntu window) without GPU compositing — a VM, or
  `WEBKIT_DISABLE_COMPOSITING_MODE=1` — flattens it (found 2026-10-28 in Linux WebKit, Playwright 1.48 image). 3D is drawn by hand.
- Six font families: subset before shipping.
- **Text over the sky** — near the horizon the sky gets light (white is only 2.5–3:1 on sunrise/day `--sky3`). In the clean sky
  the headline sits on the hill (dark) and keeps its scrim and halo; the sidebar has no scrim, so its tree is `#D6DAEA` and its words
  carry a dark halo. Measured 2026-10-28 at 02:00, 05:00, 06:36, 10:00, 13:00, 17:36 and 19:00 (95th-percentile background with the
  text hidden, halo not counted): sidebar tree 8.5–12.7:1, menu 10.3–12.3:1, headline 17.4:1, film edge 17.8:1.
- The callout shows address, system, response (the Master's timed SSH banner read, `latencyMs`), online for (since `onlineSince`,
  the first check of the current online run, as the Master saw it, not the machine's boot uptime) and when it was last checked.
- The sidebar is sticky: its copy of the sky follows the page scroll (`--scroll`, one write per frame while scrolling) so the seam never opens.
- `sky.js` builds the sky once outside `#main` and only updates it (app.js `syncSky()`), so a running light survives re-renders.
