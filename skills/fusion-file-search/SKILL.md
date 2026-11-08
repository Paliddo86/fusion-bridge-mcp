---
name: fusion-file-search
description: Choosing the RIGHT verb to find or open a Fusion cloud file - the APS server-indexed verbs (fusion_aps_search, fusion_aps_browse, fusion_aps_recent, fusion_aps_open - fast, no Fusion needed) vs the in-Fusion open verbs, plus the decision tree and why fusion_search_cloud_files / fusion_walk_cloud_tree are neutered redirect stubs. Use when searching cloud files, opening a design by name or keyword, or a search verb returns disabled:true. Trigger words - find fusion file, search cloud files, fusion_aps_search, open cloud file, find my design, search my hub, file search, cloud search, which search verb.
---

# Fusion File Search — choosing the right verb for every scenario

The bridge offers **eight ways to find a cloud file**, split across two engines that know different
things: the **APS** verbs query Autodesk's server index and need no Fusion, while the **in-Fusion**
verbs ask a running Fusion what it can see. They differ in what they search, what they cost, and
whether Fusion has to be running. This skill explains the landscape so you pick the right one.

## ⛔⛔ DEPRECATED — Do NOT use these

**These verbs are neutered in the bridge: calling them returns a safe REDIRECT stub (`disabled:true` + a router pointing at the APS verbs). They never reach Fusion anymore, so they cannot crash it - but they also do NOT search. Their in-Fusion implementations were removed after crashing Fusion with 30+ min hangs:**

- `fusion_search_cloud_files` — old in-Fusion walk: 30+ min hangs, killed Fusion
- `fusion_walk_cloud_tree` — old in-Fusion walk: 30+ min hangs + WinError 10054

**If you see them elsewhere, ignore them. Use `fusion_aps_search` (index) or `fusion_aps_browse` (tree nav) instead.**

---

## Quick decision tree

```
Just want to FIND it (no Fusion needed)?  → fusion_aps_search {query}     ← start here
  ...only if it was edited recently?      → fusion_aps_recent {limit}
  ...exploring the folder tree?           → fusion_aps_browse (one call per level)

Want it OPEN in Fusion?
  know its NAME + project?                → fusion_open_cloud_file {fileName, projectName}
  only know a keyword?                    → fusion_aps_open {query}   (finds AND opens)
  have a URN from an earlier query?       → fusion_open_by_urn {urn}
```

Fusion already running, and you want what **it** can see rather than the cloud index:
`fusion_list_cloud_projects`, then `fusion_list_cloud_files {projectName, folderPath}`.
For Fusion's MCP view of open documents: `fusion_mcp_call` with `queryType:"document"`.

**The one trap worth memorising:** the `fusion_aps_*` verbs run without Fusion, but every verb that
*opens* something needs Fusion running, because it proxies into the add-in.

---

## The verbs, side by side

"Cost" below is **relative**, from what each verb has to do, not a measured latency. Read the
[timing](#timing-what-was-actually-observed) section before quoting any number to a user.

| Verb | Engine | Cost | APS auth | Fusion | What it searches | Best for |
|------|--------|------|----------|--------|------------------|----------|
| **`fusion_aps_search`** | APS index | one indexed query | ✓ Token | ✗ No | Every cloud file, keyword match | "Find any file by name or MPN" |
| **`fusion_aps_recent`** | APS index | one list call | ✓ Token | ✗ No | Most-recently-modified designs | "What did the team just edit?" |
| **`fusion_aps_browse`** | APS index | one call **per level** | ✓ Token | ✗ No | Hub / project / folder structure | "Walk down to `Molecules` > `XRP` > `Power`" |
| **`fusion_aps_file_info`** | APS index | one lookup | ✓ Token | ✗ No | Version history, who and when | "Who last touched this, and when?" |
| **`fusion_aps_open`** | APS + add-in | query, then open | ✓ Token | ✓ YES | Finds by name, then opens it | "Open whatever matches this keyword" |
| **`fusion_open_cloud_file`** | add-in | one open | ✗ No | ✓ YES | Opens by `fileName` in a project | "I know its NAME and project" |
| **`fusion_open_by_urn`** | add-in | one open | ✗ No | ✓ YES | Opens one known URN | "I have a URN from an earlier query" |
| **`fusion_list_cloud_projects`** / **`fusion_list_cloud_files`** | add-in | one call per folder | ✗ No | ✓ YES | What a running Fusion can see | "Fusion is already open; ask it directly" |
| **`fusion_mcp_call`** | Fusion MCP | local, no cloud | ✗ No | ✓ YES | Documents in Fusion's MCP view | "What is Fusion holding right now?" |

The add-in verbs need **no APS token** (Fusion is signed in on its own) but **do** need Fusion
running. The APS verbs are the exact opposite. That trade is the main thing to reason about.

---

## Detailed Path: What Each Verb Does

### 1. `fusion_open_cloud_file` — open by NAME, inside a running Fusion

**Use when:** you know the file's **name** and (ideally) its project, and you want it open.

⚠️ **There is no `filePath` argument.** This verb finds by name, not by a slash-separated path.
Passing `{"filePath": "Molecules/XRP/Power/BQ25792.fprj"}` fails with an unhelpful error. The real
arguments come from the add-in's `handle_open_cloud_file`:

```bash
adom-bridge fusion_open_cloud_file '{
  "fileName":    "BQ25792",            # required
  "projectName": "Main",               # optional, defaults to the active project
  "folderPath":  "Molecules/XRP/Power" # optional, defaults to the project root
}'
```

**Tradeoffs:**
- ✓ No APS token needed: Fusion is already signed in on its own
- ✓ Retries automatically on Fusion's `InternalValidationError`, which it throws often on the first
  attempt after a recent close
- ✓ Detects a post-open modal and auto-screenshots, so a blocked open reports the real cause
  (`dialogBlocking: true`) rather than a misleading "add-in not responding"
- ✗ **Requires Fusion running.** It proxies into the add-in; it is not an APS call
- ✗ Cloud files can take 30+ seconds to download and open, so budget for it

**When it fails:** the response `_hint` sends you to `fusion_list_cloud_projects` (see the projects
available), then `fusion_list_cloud_files` with a known project name (browse its folders). Take
that route rather than guessing at names.

> **Electronics rule.** Open the **project** (`EcadDesignProductType`), never the schematic or
> `.brd` directly: the schematic, 2D board and 3D board are *views inside one design*. Opening a
> child gives you an isolated or empty view. The open verbs reprimand you in `_hint` when you get
> this wrong, so read it.

---

### 2. `fusion_aps_search` — The "find anything" workhorse

**Use when:** you have a keyword (a name or an MPN) and no idea where the file lives.

```bash
adom-bridge fusion_aps_search '{"query":"BQ25792","limit":10}'
# Response: { "count": 9, "results": [{name, id, projectName, lastModified}, ...] }
```

**Tradeoffs:**
- ✓ Server-indexed: one query, versus the 30-minute folder walk the disabled verbs did
- ✓ Searches ALL cloud files at once (no folder traversal)
- ✓ Returns rank-ordered results (newest/best matches first)
- ✓ No Fusion needed
- ✗ Needs APS token (call `fusion_aps_signin` if expired)
- ✗ Keyword matching, so a typo returns nothing rather than a near miss
- ✗ Crosses the network, so it can take tens of seconds. Never poll it in a loop

**What the response gives you:**
- `name` — the file display name
- `projectName` — which hub/project it lives in
- `id` — the file ID (use for `fusion_aps_file_info` to get versions)
- `lastModified` — when it was last edited

**When it fails:**
- `aps_not_signed_in` → sign in again (see the error table below before you touch a browser)
- `count: 0` → no matches. Try a shorter fragment: matching is on the name, so `BQ25792` finds
  `BQ25792` but a mistyped or over-specified query finds nothing

---

### 3. `fusion_aps_recent` — what the team touched last

**Use when:** you want the most-recently-modified designs across the hub, newest first. This is a
server query, not a local cache, and it has no time window: `limit` (default 25) is what bounds it.

```bash
adom-bridge fusion_aps_recent '{"limit":25}'
# Response: { "count": 25, "results": [{name, id, projectName, lastModified}, ...] }
```

**Tradeoffs:**
- ✓ Cheap: one list call, no query parsing
- ✓ No Fusion needed
- ✓ Shows workflow (what people are actually using)
- ✓ No keyword parsing (exact list, no false negatives)
- ✗ Bounded by `limit` (default 25), so an older file simply will not appear. Raise `limit` or
  switch to `fusion_aps_search`
- ✗ Sorted by modification time, not relevance (design A might be #1 just because someone opened it)

**When to use:**
- "Show me recent designs so I can pick one to demo"
- "What did the team just update?" (good for discovery)

---

### 4. `fusion_aps_browse` — Tree navigation (folder by folder)

**Use when:** You want to navigate the folder structure (e.g. "show me what's in Molecules/XRP/Power").

```bash
# Browse root (hubs)
adom-bridge fusion_aps_browse '{}'
# Response: { "items": [{name, type:"hub"|"project"|"folder"|"file", id}, ...] }

# Browse a folder
adom-bridge fusion_aps_browse '{"folderId":"urn:adsk.wipprod:fs.folder:..."}'
# Response: same structure
```

**Tradeoffs:**
- ✓ Shows full folder tree (helps you understand hierarchy)
- ✓ No Fusion needed
- ✓ Lets you explore without guessing names
- ✗ One round trip PER LEVEL, so a deep path costs several network hops
- ✗ Requires folder/project IDs from a prior call (not user-friendly unless you're chaining calls)
- ✗ Can't search by keyword (only linear folder walk)

**Typical use:**
```
1. fusion_aps_browse {}  → list hubs
2. Find "Main" hub
3. fusion_aps_browse {"hubId":"..."} → list projects in Main
4. Find "Adom Electronics" project
5. fusion_aps_browse {"projectId":"...", "folderId":"..."} → drill into Molecules
6. See BQ25792.fprj in the listing
```

**When it fails:**
- Empty `items` → you are at a leaf, or the id you passed is stale. Re-browse from the level above
  rather than guessing a replacement id

---

### 5. `fusion_mcp_call` — Fusion-local search (via MCP)

**Use when:** Fusion is OPEN and you want to query what it KNOWS (open docs + MCP cache).

```bash
# Query Fusion's document cache
adom-bridge fusion_mcp_call '{"tool":"fusion_mcp_read", "arguments":{"queryType":"document"}}'
# Response: { "result": { "documents": [{name, type, ...}, ...] } }
```

**Tradeoffs:**
- ✓ Runs on the user's own machine, so no cloud round trip at all
- ✓ No internet latency (runs on the user's machine)
- ✓ No auth needed (Fusion-local)
- ✗ ONLY searches Fusion's cache (recently opened docs)
- ✗ Requires Fusion to be RUNNING
- ✗ Requires MCP to be ENABLED (call `fusion_mcp_enable` first if it's off)
- ✗ Limited to what Fusion knows (won't find archived files)

**When to use:**
- "The user has Fusion open with the board; let me query Fusion's open docs"
- "What files did Fusion recently touch?"
- When you want to avoid cloud round-trips (good for demos/offline work)

**Typical flow:**
```
1. Check if MCP is enabled: fusion_mcp_status
2. If not, enable it: fusion_mcp_enable
3. Query: fusion_mcp_call {tool:"fusion_mcp_read", arguments:{queryType:"document"}}
4. Filter results client-side for "BQ25792"
```

---

### 6. `fusion_open_by_urn` — URN express lane

**Use when:** You already have a file's URN (from a prior APS call or a saved link).

```bash
adom-bridge fusion_open_by_urn '{"urn":"urn:adsk.wipprod:fs.file:vf.DqSW..."}'
# Response: { "success": true, "output": "Opened: BQ25792" }
```

**Tradeoffs:**
- ✓ Direct open (no search needed)
- ✓ Works even if filename changed (URN is immutable)
- ✗ Requires knowing the URN upfront
- ✗ Needs Fusion to be RUNNING
- ✗ Needs APS token

**When to use:**
- Saved workflows ("I have a URN from yesterday; reopen it")
- Chaining calls ("Get the URN from APS, then open it via URN")

---

## Scenarios: Which Verb to Choose

### Scenario 1: "Open the BQ25792 board from the cloud"

```
Fusion running?  → fusion_start first (every OPEN verb proxies into the add-in)
Know the name?   → fusion_open_cloud_file {fileName:"BQ25792", projectName:"Main"}
Only a keyword?  → fusion_aps_open {query:"BQ25792"}   (searches, then opens)
```

Open the electronics PROJECT, then switch views with `fusion_show_schematic` /
`fusion_show_2d_board` / `fusion_show_3d_board`.

### Scenario 2: "Find any battery charger board in the cloud"

```
Need keyword search? "battery" or "charger" → YES
Path unknown → NO
↓
→ fusion_aps_search {query: "charger", limit: 10}
   (one indexed query, shows all matches)
```

### Scenario 3: "What's everyone been editing lately?"

```
Want recency, not keyword → YES
↓
→ fusion_aps_recent {limit: 25}
   (one list call, shows what the team is working on)
```

### Scenario 4: "Browse Molecules/XRP to see what's there"

```
Want to explore folder structure → YES
↓
→ fusion_aps_browse {folderId: "..."}
   (hierarchical, good for discovery)
```

### Scenario 5: "The user has Fusion open. What's in their current session?"

```
Fusion is OPEN and I want local results → YES
MCP enabled? → maybe (check fusion_mcp_status first)
↓
→ fusion_mcp_call {tool: "fusion_mcp_read", arguments: {queryType: "document"}}
   (local to the user's machine, no cloud)
```

### Scenario 6: "I have the URN from yesterday. Reopen it."

```
Have a saved URN → YES
↓
→ fusion_open_by_urn {urn: "urn:..."}
   (direct, fast, even if name changed)
```

---

## Timing: what was actually observed

**Do not quote latency figures you have not measured.** An earlier draft of this skill carried a
table of confident millisecond numbers that were never measured, and the two calls that *were*
timed came back an order of magnitude slower than the table claimed. Invented numbers are worse
than no numbers, because an AI will plan around them.

Every bridge response carries a **`durationMs`** field: the bridge's own view of the call, with the
relay hop excluded. That is the number to trust, and it costs you nothing to read it.

Single observations from one session on one laptop, over the relay, warm token, for shape only:

| Call | Wall time | Note |
|---|---|---|
| `fusion_aps_search {query:"BQ25792"}` | ~50s | Returned "Found 9 match(es) across 16 project(s)". Wall time includes the relay round trip. |
| `fusion_aps_recent {limit:5}` | ~11s | Returned 5 designs. |
| `fusion_aps_get_browser` | 24ms (`durationMs`) | A local read with no cloud hop, for contrast. |

Two honest conclusions, and no more than two: **anything touching APS crosses the network and can
take tens of seconds**, so budget for it and do not poll it in a loop; and **a local read is
roughly instant**. Everything else about relative speed follows from the Cost column above, which
is reasoning about the work each verb does, not measurement.

---

## Common Errors & Fixes

These are the `errorCode` values the bridge really emits. There is no `not_found` and no
`permission_denied`; do not branch on codes you have not seen in a response.

| `errorCode` | Verb | Fix |
|-------------|------|-----|
| `aps_not_signed_in` | any `fusion_aps_*` | `fusion_aps_signin`. **Check `fusion_aps_get_browser` first** and never pass `allowDefaultBrowser` when a browser is already remembered |
| `aps_not_configured` | any `fusion_aps_*` | `fusion_aps_set_client_id` with the org's APS client id |
| `aps_app_missing_product` | any `fusion_aps_*` | The APS app lacks the Data Management API product. The response carries the recovery recipe |
| `fusion_not_running` | any add-in verb | `fusion_start` |
| `fusion_not_installed` | any add-in verb | `fusion_install_fusion` |
| `fusion_addin_not_responding` | any add-in verb | Usually a modal blocking Fusion's main thread. Screenshot, clear the dialog, retry |
| `main_thread_busy` | any add-in verb | Fusion is mid-operation. Wait and retry |
| `disabled` | `fusion_search_cloud_files`, `fusion_walk_cloud_tree` | They crash Fusion. Use `fusion_aps_search` / `fusion_aps_browse` |

`token_expired` is not an `errorCode`, it is a **`stage`** on the sign-in flow, meaning the refresh
failed and you should sign in again.

---

## When to Chain Verbs

Sometimes you need more than one call:

### Example 1: "Find the board and open it"
```
1. fusion_aps_search {query:"BQ25792"}           → get results + projectName
2. fusion_open_by_urn {urn: results[0].id}       → open it
```

### Example 2: "Get file history"
```
1. fusion_aps_search {query:"BQ25792"}           → get results + id
2. fusion_aps_file_info {query:"BQ25792"}        → get version history
3. fusion_aps_versions {query:"BQ25792"}         → get all versions
```

### Example 3: "Browse and open"
```
1. fusion_aps_browse {}                                → list hubs
2. fusion_aps_browse {hubId:"..."}                     → list projects
3. fusion_aps_browse {projectId:"...", folderId:"..."} → browse down to the folder
4. fusion_open_cloud_file {fileName:"...", projectName:"..."}   → open it (Fusion must be running)
```

---

## Hints the Bridge Provides

Every search verb includes a `_hint` field explaining the NEXT action:

- A failed open sends you to `fusion_list_cloud_projects` / `fusion_list_cloud_files`
- A blocked open reports `dialogBlocking: true` plus a screenshot, naming the real dialog
- Opening a non-`EcadDesignProductType` file is reprimanded in `_hint`: reopen the parent project
- A `_timeoutHint` appears on long verbs: a timeout is usually NOT failure. The work keeps running
  on the bridge, so poll `fusion_get_app_state` instead of re-issuing the call

**Always read the `_hint` — it tells you what to do next.**

---

## Summary: One Rule

> **Default to `fusion_aps_search` for any "find a file" task.** One indexed query, reliable,
> and requires nothing but a keyword. Use the others only when you have a specific reason:
> known path (→ direct open), recent items (→ cache), browsing folders (→ tree nav), or
> Fusion already open (→ MCP local).
