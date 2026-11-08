---
name: fusion-ralph-test-via-wiki
description: >
  How the fusion bridge tests ITSELF the way a real user drives it: open a wiki component
  page in a browser you own, press Connect and Create Fusion Library, and watch every
  fusion_* verb the page sends while a live feed shows what Fusion is actually doing. The
  end-to-end ralph loop, plus the traps that cost an afternoon (synthetic clicks the widget
  ignores, a hidden pre-connect status node, two racing chains, a restart nobody could see).
  Read before claiming any bridge change works, and after any change to the import lane.
  Trigger words - ralph test fusion, test the wiki export, quick connect, create fusion
  library button, watch the wiki drive fusion, webcontrol.js, wc-connect, fusion_live_feed,
  end to end test, did the import actually work.
discovery_triggers:
  - "ralph test fusion"
  - "test the wiki fusion export"
  - "fusion_live_feed"
  - "watch fusion during import"
---

# Ralph-testing the bridge through the wiki (the way users actually use it)

**Why this exists.** On 2026-08-28 the bridge passed every stubbed test and every direct-verb
test, then a real press of the wiki's Create Fusion Library button restarted Fusion under the
user after he had pressed "I'll handle it", showed him "launching Fusion" over a Fusion that
was plainly open, and left four Untitled tabs behind - and no screenshot ever showed him any
of it. *"verified by driving the verb"* and *"verified by watching the feature"* are
different tests. Only the second proves anything to a user. This is the second.

## The loop

```
1. python3 tools/live_feed_drain.py --target <Box> --channel fusion-live &   <- SEE Fusion first
   (starts fusion_live_feed on the box and drains its ring buffer into shotlog, tagged)
2. pup_open_window   {sessionId, url: wiki.adom.inc/<owner>/<part>}   (your own window)
3. tap the page's fetch (below) so every verb it sends is recorded
4. toggle the lane to Fusion; press Connect; confirm wc-status says "Connected"
5. press Create Fusion Library
5b. ACCEPT THE TRUST MODAL. The press does NOT start the import - it opens
    "Open <owner>/<part> in Fusion? ... Only continue for a publisher you trust.
    [Cancel] [Open in Fusion]" (`.wc-modal-overlay`, ok button `.wc-modal-ok`).
    Nothing happens until it is accepted. Leave "Don't ask again" to the human.
6. watch THREE witnesses: the page's verb log, fusion_prove_result, and the live feed
7. read the terminal state, the partMetadata, and the proof frames
8. fusion_close_all_documents {closeUntitled:true, force:true}; pup_close_window
```

Step 1 is not optional. The whole failure above was invisible because nothing looked.

## Tap the page so you can see what it sends

The page drives the bridge through `POST /command`. Record every call:

```js
window.__wcnet=[];const of=window.fetch;window.fetch=async function(u,o){
  const t=Date.now();let body="";try{body=(o&&o.body)?String(o.body).slice(0,260):""}catch{}
  let r;try{r=await of.apply(this,arguments);window.__wcnet.push({t,u:String(u),m:(o&&o.method)||"GET",body,st:r.status});return r}
  catch(e){window.__wcnet.push({t,u:String(u),m:(o&&o.method)||"GET",body,err:String(e)});throw e}}
```

Then `JSON.stringify(window.__wcnet.filter(x=>/\/command/.test(x.u)).map(x=>JSON.parse(x.body).verb))`
gives you the verb sequence. Yesterday's real sequence for one press:
`fusion_create_library` -> `fusion_prove_result` (polled). That is the contract the wiki
actually uses; anything the bridge changes in it must be checked here.

## The traps (each cost real time)

**The press opens a consent modal, and your poll will read "nothing happened".** Four runs
in a row on 2026-08-28 showed zero traffic after the press, and I blamed synthetic clicks,
stale closure state, and a fresh reload before looking at the DOM: every press had opened
`.wc-modal-overlay` ("Open ray/dac80504 in Fusion? Only continue for a publisher you
trust") and was waiting. That is quick-connect doing exactly the right thing. Read the DOM
for a modal BEFORE theorising about why a click "did nothing".

**`pup_click` does not reach the widget.** The Connect and Create buttons use delegated
handlers; pup's synthetic click never fired them, so "pressed" changed nothing. Dispatch a
real DOM event instead:
`[...document.querySelectorAll("button")].find(b=>b.innerText.trim()==="Create Fusion Library").click()`

**The visible status text lies.** `.wc-status` has a hidden pre-connect node ("Press a button
to connect...") that is still in the DOM after connecting. Read the VISIBLE one, or read
`.wc-dot.is-up` and `localStorage.adomWebControlConnected`. The page was connected for ten
minutes while the text said it was not.

**The lane toggle is per-page state.** `localStorage.adomWebControlEda` is `kicad` or
`fusion`; Connect routes to whichever is set. Set it before pressing anything:
`[...document.querySelectorAll(".wc-eda-btn")].find(b=>b.innerText.trim()==="Fusion").click()`

**Two presses = two chains.** Before 1.9.239 a second Create started a second import that
raced the first for one Fusion. Now it attaches to the running `runId`. If you see two runIds
in the poll log, you are on an old bridge.

**"launching Fusion" over an open Fusion.** Before 1.9.239 the chain named its first stage
"launching" whether it launched or merely restarted a Fusion whose add-in had gone silent.
The stage now says which; `stageReason` is `fusion_not_running` or `addin_not_answering`.

**A bridge install re-syncs the add-in under a running Fusion.** That is what silenced the
add-in in the first place. After `bridge_install`, expect `ready:false` until Fusion is
restarted - and the user owns that restart if they said so.

## Reading the three kinds of frame

`fusion_live_feed` keeps a box-side ring buffer; `tools/live_feed_drain.py` publishes it to shotlog as `[main #n]` (the window each tick - the video)
and `[popup #n.m]` (each owned dialog/panel over it, tied to its tick - Fusion draws these as
separate windows a main capture omits). The chain's evidence frames are `kind:proof` with a
step label. A Browser panel, the main frame, and a proof shot are three different things; a
viewer must never have to guess. The wiki may not render the feed; the bridge owns it anyway.

## A completed run looks like this (AdomLapper, 2026-08-28, run 8f3a376d6782)

Page https://wiki.adom.inc/ray/dac80504, Fusion lane, Create Fusion Library, trust modal
accepted. The page sent: fetch .lbr, `write_file`, fetch .step, `write_file`,
`fusion_create_library`, then `fusion_prove_result` every few seconds. The chain reported:

- `stage: done`, `prove.proven: true`, `partsProven 1 / partsTotal 1`, `evidenceCount 4`
  (symbol, footprint, package3d, ganged; every frame `kind: proof`, `part: DAC80504`).
- `bind` 57s, `wip_urn` minted, binding written into `part.lbr` on disk (a `.prebind`
  backup beside it) BEFORE the reopen, so closing the modified `part*` tab loses nothing.
- `partMetadata.ok: false` with the honest text: "1 of 1 component has no manufacturer part
  number", distributor coverage 0/0/0, `userAction` pointing upstream. The import still
  completed; the gap bubbled up. That is the contract.
- `scratchDocsClosed: 3`, `libraryDocClosed: true`, `fusion_get_app_state` documents `[]`.
- The page's third `.wc-res` (the one under the Create button, not the first one on the
  page) read "library created in Fusion". The page rendered no proof thumbnail yet (#57).

## The feed can spin without being live (fixed 1.9.245)

`fusion_live_feed` status said `running:true, tick:199, buffered:0` for ten minutes while
the run happened, and the drainer landed nothing. Tick counted ATTEMPTS and every capture
failure died in `except: pass`. As of 1.9.245 status and frames carry `captured`, `noHwnd`,
`lastError`, `lastCaptureAt`, `healthy`, and an `_honesty` line when the tick climbs with no
frame. The drainer prints that line. Rule: a feed is live when `captured` climbs, never
when `tick` does. Read `healthy` before you tell anyone they have a video.

## A proof frame is evidence only if it is measured (fixed 1.9.249)

Run 9bf511091a96 scored `proven` with a 237x39 symbol frame (a title-bar sliver) and a
570x615 footprint frame (a stray window). The capture gate read `width`/`height` that the
screenshot never reports (it reports `fullWidth`/`fullHeight`) and defaulted to PASS when
they were missing. Now every frame's size comes from the file header when the shot does not
say, a frame under 400x300 is `wrong_window_captured`, and an unmeasurable frame is refused.
Rule: when you check evidence, pull one frame and look at its pixels; `ok:true` on a frame
nobody measured is the same false pass as `stage:done` with zero frames.

## Dev checklist before saying "it works"

- [ ] Ran the loop above on a real box, not a stub, and read the frames (their pixel SIZE too: a 237x39 frame is a title bar, not a symbol).
- [ ] Accepted the trust modal (and looked for one whenever a press "did nothing").
- [ ] The verb sequence the page sent matches the documented lane.
- [ ] `fusion_prove_result` reached `done` with `verdict: proven` and `partsProven == partsTotal`.
- [ ] `partMetadata` reported the part-number state (it never blocks, it must still show).
- [ ] `fusion_live_feed` status shows `healthy: true` and `captured` climbing, or you said the feed was not live.
- [ ] No Untitled documents left behind (`fusion_get_app_state` shows `[]`).
- [ ] Nothing restarted Fusion without the user's consent (ledger has no unexplained restart).
- [ ] Closed your own pup window; never touched the user's.
