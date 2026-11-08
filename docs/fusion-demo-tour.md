# fusion_demo: the showcase tour and the onboarding flow

One verb, two jobs, chosen by the `mode` argument.

## Tour mode (the showcase): `fusion_demo {"mode": "tour"}`

A one-call, self-contained showcase that walks every electronics content type Fusion
has, using sample data the bridge ships (so it works on a box with zero content), and
reports structured progress a front-end can render as a real progress bar.

What it visits, in order:

| Step | What the user sees |
|---|---|
| stage | The bundled sample library staged onto the box |
| library | The library opened in the Electronics Library workspace |
| symbol | The schematic symbol, full pinout, in the symbol editor |
| footprint | The footprint, pads and layers, in the footprint editor |
| device | The component view: symbol + package mapping table |
| 3d-package | The 3D chip in the Package3D environment |
| open-design* | The cloud electronics design (project) opening |
| schematic* | Schematic view of the real board |
| 2d-board* | 2D board layout |
| 3d-board* | The 3D board |

Steps marked * run only when a `query` argument names a cloud design (needs a
signed-in Fusion session); without it the tour is library-only.

### The call contract

Start (returns immediately, work runs async):

```json
fusion_demo {"mode": "tour", "query": "BQ25792", "includeDesign": true}
-> {"demoId": "tour-xxxx", "estimatedSec": 286, "steps": [{"step": "...", "label": "...", "estimatedSec": n}, ...]}
```

Poll about once a second:

```json
fusion_demo_progress {"demoId": "tour-xxxx"}
-> {"percent": 42, "step": "3d-package", "stepLabel": "3D chip: the Package3D environment",
    "elapsedSec": 121, "estimatedSec": 286, "done": false}
```

The final poll (`done: true`) additionally carries:

- `shots`: the screenshot gallery, each item `{title, image (base64), path}`
  (pass `imagesAs: "paths"` to skip the base64 encoding)
- `narrate`: a one-line human summary to display or speak
- `failedSteps`: step keys that were skipped or died (a tolerant tour keeps going)
- `warnings`: the reasons behind any skipped steps

This shape is the Adom Bridge web-control harness contract (live since ab 2.0.39, the
same runner that drives `kicad_demo`), so the demo plugs into `POST /command/stream`
with no ab-side work.

### Etiquette

The tour runs **entirely in the background** by default; the user keeps their
foreground and can type in other AI threads while it works. Pass `foreground: true` on
the start call only when the user explicitly wants to watch Fusion live. See the
fusion-background-etiquette contract.

### Prerequisites

- Fusion must be running (`fusion_start` first; it launches in the background too).
- The design-side steps need a signed-in Fusion session; the library side needs nothing.

## Onboarding mode (the default): `fusion_demo {}`

The staged, resumable first-time-user flow that Hydrogen Desktop's installer drives:
finish the Autodesk sign-in (via the user's native browser, never pup), set up APS
cloud search and prove it with a live search, then open a real project and walk
schematic, 2D board, 3D board. Each call advances as far as it safely can and returns
`stage / done / narrate / screenshots` plus a `_hint` with the exact next action; the
caller loops until `done: true`. Full playbook: the `fusion-demo` skill.

Bare `fusion_demo` stays this onboarding flow permanently; the tour lives behind
`mode: "tour"` so installer callers never break.
