# SSH Control Center — Design System "Engraved Violet" (v5.1 Dusk)

One flat dusk-violet field (calm enough for all-day use), acid-green accent, huge condensed type, metal drawn as engraving lines, one holographic foil per screen.
Reference mockup: `docs/design/design-reference-v5.html` — copy its layout, spacing and effects.

## Golden rules (do not break)

1. **One flat violet background** (`--violet`). No dark mode, no glows, no blurred gradients, no glassmorphism.
2. **Square corners everywhere** (`border-radius: 0`). No pill buttons, no rounded cards.
3. **No boxes for content.** Separate with hairline rules (1px white, or white at 35%). Only the illustration plate and the terminal get a fill.
4. **Metal = engraving.** Metallic look comes from fine line patterns (`.eng`, `.sphere`, `.rays`), never from shiny chrome gradients or drop shadows.
5. **Green is the only accent.** Use it for selection (a full green row in content lists; a thin 2px acid bar in the navigation tree so the tree stays quiet), live indicators, and the accent outline button. Never as a large background except a selected list row.
6. **Hologram foil at most once per screen** (a sticker/seal or a thin ring).
7. **Type is the hero:** condensed display at extreme sizes (Light 200 for words, Black 900 for big numbers).
8. **Accessible:** `--white` text on violet only (≥ 7:1); muted text is `--lilac` (also ≥ 7:1), never lower. Focus: `outline: 2px solid var(--acid); outline-offset: 3px`. Targets ≥ 44 px. Animations stop under `prefers-reduced-motion`.
- **Motion with meaning (topology only):** a short acid light runs down from this Master and out along each path that has online hosts (and across a merged Master to its hosts); online status squares breathe like an LED; a card flashes once (acid or rose) when its host changes state. Opacity/transform only, no glow, and nothing moves under `prefers-reduced-motion`. Everything else stays static.

## Tokens

```css
:root {
  --violet: #2E1873;   /* page — "Dusk": hue kept, saturation lowered for long sessions */
  --plate:  #361E85;   /* illustration plate */
  --deep:   #160A3D;   /* terminal, ink on foil */
  --ink-on-green: #24105E;
  --white:  #ECE8FA;   /* text: a violet-tinted white, softer than #FFF */
  --lilac:  #BCB1E8;   /* muted text, labels (7.1:1) */
  --lilac-2:#A396DA;   /* terminal output */
  --acid:   #A6F26C;   /* the one accent (10.4:1) */
  --amber:  #FFD45C;   /* high load */
  --rose:   #FF9CB8;   /* offline (7.2:1) */
  --rule:   rgba(236,232,250,.28);
}
```

## Typography

Google Fonts: `Big+Shoulders+Display:wght@200;300;700;900`, `Geist:wght@400;500;600`, `Geist+Mono:wght@400;500`.

| Role | Font | Size / weight |
|---|---|---|
| Hero number | Big Shoulders Display + `.eng` | 150–240 px / 900, line-height .82 |
| Hero words, section titles | Big Shoulders Display | 56–96 px / 200–300 |
| Logo (stacked, centered) | Big Shoulders Display | 40 px / 300, line-height .86 |
| Nav, buttons, status | Big Shoulders Display | 18–22 px / 500–700, letter-spacing .1–.16em, UPPERCASE |
| Host names | Big Shoulders Display | 26 px / 700 |
| Body | Geist | 15–19 px / 400, line-height 1.5 |
| Labels ("FIG. 01 — …"), IPs, terminal | Geist Mono | 11–13 px, letter-spacing .14–.18em for labels |

Thai (ไทย): Bai Jamjuree (300 / 500 / 700) for display text and Anuphan for body and labels, bundled as Thai-only subsets so each stack keeps its Latin face and Thai falls through to the Thai one. Thai text has no letter-spacing (tone marks drift when letters are spaced), line-height 1.2 for h1/h2 and 1.3 for buttons and headings, and labels at 13 px instead of 11 px.

## Effects (copy as-is)

```css
/* Engraved metal numerals */
.eng{background:repeating-linear-gradient(180deg,transparent 0 2px,var(--violet) 2px 4px),linear-gradient(180deg,#FFFFFF 0%,#F1EDFF 38%,#B9A8FF 52%,#FFFFFF 60%,#9CFF57 100%);-webkit-background-clip:text;background-clip:text;color:transparent}

/* Sunburst rays (white) + sparse green rays */
.rays{position:absolute;inset:0;background:repeating-conic-gradient(from 0deg at 50% 50%,rgba(255,255,255,.7) 0deg .3deg,transparent .3deg 2.4deg);-webkit-mask:radial-gradient(circle,transparent 0 30%,#000 31%,#000 52%,transparent 70%);mask:radial-gradient(circle,transparent 0 30%,#000 31%,#000 52%,transparent 70%)}
.rays-g{position:absolute;inset:0;background:repeating-conic-gradient(from 1deg at 50% 50%,#9CFF57 0deg .5deg,transparent .5deg 12deg);-webkit-mask:radial-gradient(circle,transparent 0 44%,#000 45%,#000 62%,transparent 80%);mask:radial-gradient(circle,transparent 0 44%,#000 45%,#000 62%,transparent 80%)}

/* Engraved chrome sphere (hatching + contour lines) */
.sphere{position:absolute;border-radius:50%;background:repeating-linear-gradient(0deg,#FFFFFF 0 1.3px,transparent 1.3px 5px);-webkit-mask:radial-gradient(circle at 34% 28%,#000 0 14%,rgba(0,0,0,.75) 32%,rgba(0,0,0,.22) 62%,rgba(0,0,0,.55) 86%,#000 97%,transparent 100%);mask:radial-gradient(circle at 34% 28%,#000 0 14%,rgba(0,0,0,.75) 32%,rgba(0,0,0,.22) 62%,rgba(0,0,0,.55) 86%,#000 97%,transparent 100%)}
.contour{position:absolute;border-radius:50%;background:repeating-radial-gradient(circle at 34% 28%,rgba(255,255,255,.55) 0 1px,transparent 1px 9px);-webkit-mask:radial-gradient(circle at 60% 70%,#000 0 30%,transparent 70%);mask:radial-gradient(circle at 60% 70%,#000 0 30%,transparent 70%)}

/* Hologram: thin spinning ring + foil sticker */
.holo{background:conic-gradient(from 0deg,#9CFF57,#7CF6FF,#FFFFFF,#FF9CF2,#C3A6FF,#9CFF57)}
.ring{position:absolute;border-radius:50%;-webkit-mask:radial-gradient(circle,transparent 0 63%,#000 63.4% 66%,transparent 66.4%);mask:radial-gradient(circle,transparent 0 63%,#000 63.4% 66%,transparent 66.4%);animation:spin 20s linear infinite}
.foil{background:linear-gradient(115deg,#9CFF57 0%,#7CF6FF 22%,#FFFFFF 40%,#FF9CF2 58%,#C3A6FF 76%,#9CFF57 100%);background-size:220% 100%;animation:foil 7s ease-in-out infinite;color:var(--deep)}

@keyframes spin{to{transform:rotate(360deg)}}
@keyframes foil{0%,100%{background-position:0% 50%}50%{background-position:100% 50%}}
@media (prefers-reduced-motion:reduce){.ring,.foil{animation:none}}
```

## Buttons (square)

| Kind | Style | Hover |
|---|---|---|
| Primary | bg `--white`, text `#3A07C8`, 56 px tall | bg `--acid` |
| Secondary | 1px `--white` border, transparent, `--white` text | bg `--acid`, text `--ink-on-green` |
| Accent | 1px `--acid` border, acid text | bg `--acid`, text `--ink-on-green` |

All: Big Shoulders Display 700, 18–22 px, letter-spacing .1em, uppercase, transition 180 ms ease-out.

## Layout (main screen)

The original v5 mock (3-column header, hero, fleet list) is superseded in the app by this layout, chosen so the centre panel carries the data:

1. **Sidebar** — full height at the left edge, 236 px, a hairline on its right. Top to bottom: accent "+ Add host", the one host filter, the menu (Inventory ▾ with the host tree under it, Masters, Service), and the current Master with Refresh and the mono version at the bottom. Active menu item = white text, hover tint and a 2px acid bar on the left; the selected tree row uses the same bar. Tree type is small and muted; Masters at the top level, folders and hosts under them. No datacenters.
2. **Inventory** — the header scene as a thin banner (150 px; its circle is the logo ring, its towers light up for hovered hosts), one summary line (all online, or the hosts that need a look as links), then the top-down topology: this Master, a "Direct from" group and one "Through <Master>" group per merged Master. Each group is a panel (1px rule, faint lilac tint) holding darker cards, so the groups read at first glance.
3. **Host / Master pages** — heading, tabs (Summary · Jobs & logs · Files · Containers · Monitoring), content. No banner.

## Status

Status is a **round dot** in its colour beside a white label (7 px in the sidebar tree, 9–10 px elsewhere).

| State | Dot | Label |
|---|---|---|
| Online | `#9CFF57` | Online (white) |
| High load | `--amber` | High load |
| Offline | `--rose` | Offline |
| Checking | empty ring, `--lilac` | Checking… |
| Selected | `--acid` row / 2px acid bar in the tree | — |
