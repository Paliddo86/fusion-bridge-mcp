---
name: fusion-dark-mode
description: Put Fusion 360 fully into dark mode - all THREE layers (UI theme via fusion_set_preference, Electronics 2D editor themes, and the 3D viewport canvas which is an ENVIRONMENT under Display Settings, not a preference). Use when the user says enter dark mode, dark background, white canvas, blinding white background, or a screenshot shows a white 3D canvas. Trigger words - dark mode, enter dark mode, dark background, white background, white canvas, canvas background, 3d background, environment, river rubicon, dark sky, photo booth, fusion theme, darkblue.
---

# Fusion dark mode - the full "enter dark mode" recipe

When a user says "enter dark mode", "make Fusion dark", "get rid of the white background",
or a screenshot comes back with a blinding white 3D canvas: Fusion's dark mode is THREE
separate settings in three different places. Adom is a dark system, so set all three.

Trigger words: dark mode, enter dark mode, dark background, white background, white canvas,
canvas background, 3d background, environment, river rubicon, dark sky, photo booth.

## The three layers

| Layer | What it darkens | Where it lives | How to set it |
|---|---|---|---|
| UI theme | Ribbon, panels, dialogs | Preferences > General > Theme | `fusion_set_preference {"theme":"darkblue"}` - one API call, no UI |
| Electronics 2D editors | Schematic + PCB layout canvas | Preferences > General > Electronics | Theme: Dark radio for BOTH Schematic and PCB (often already dark) |
| **3D viewer canvas** | The Design / 3D PCB viewport background | **NOT in Preferences.** Display Settings > Environment | Pick **River Rubicon** (dark, the Adom house pick) or **Dark Sky** (darker) |

The 3D canvas is the one everyone hunts for in Preferences and never finds. It is an
ENVIRONMENT, set from the viewport's own Display Settings menu (John Lauer, 2026-08-09,
after this exact hunt burned a session).

## Layer 3 in detail: the 3D canvas environment

Two ways to reach the same menu:

1. The **monitor icon** in the floating toolbar at the BOTTOM CENTER of the viewport
   (Display Settings), or
2. **Right-click empty 3D canvas** and find Display Settings in the context menu.

Then: `Display Settings > Environment >`

```
Theme Environments        Legacy Environments
  Photo Booth   (LIGHT      Infinity Pool
                 default)   Grey Room
  River Rubicon (dark)      Tranquility Blue
  Dark Sky      (darkest)
```

**River Rubicon** is the Adom standard: a dark blue-gray canvas that matches the darkblue
UI theme, keeps soft ground shadows, and makes a green PCB pop for screenshots. Dark Sky
is darker still if the user wants near-black.

The environment applies to the ACTIVE viewport's document. Verify by screenshot, never
assume: `desktop_screenshot_window {hwnd}` and LOOK at the canvas color.

## API and text-command notes

- `fusion_set_preference {"theme":"darkblue"}` works live, no restart. The values `dark`
  and `darkgray` are accepted by the API but NOT shipped in every Fusion build; the set
  reports ok while `activeTheme` stays put. Read back `fusion_get_preferences` to confirm.
- There is NO API or preference key for the 3D canvas environment (verified against
  generalPreferences/apiPreferences on Fusion 2703+). The Display Settings menu is the
  only route; drive it with clicks.
- `Commands.Start PreferencesCommand` returns Ok even though the dialog opens as a
  SEPARATE TOP-LEVEL WINDOW titled "Preferences" (not an owned popup of the main window).

## Traps when driving the Preferences dialog (all hit live, 2026-08-09)

1. **The dialog is a top-level window now.** Find it in `desktop_list_windows` by title
   "Preferences"; do not wait for an owned popup on the Fusion hwnd.
2. **A maximized window (yours or the user's) eats your clicks.** Screen-space clicks land
   on whatever is TOPMOST at that point, and they still return ok. If the dialog is buried,
   make it topmost WITHOUT activation first (SetWindowPos HWND_TOPMOST + SWP_NOACTIVATE),
   click, then drop topmost. Never minimize the user's windows to get at it.
3. **UIA lies here.** `desktop_ui_click` on a tree item and `desktop_ui_expand` both report
   ok without doing anything visible. Trust only a fresh screenshot.
4. **Expanding General needs a real click on the expander arrow** (image-space click via
   the capture's coordMap). Right-arrow-key expansion does not work either.
5. **Never click "empty" pane space to focus it.** Prefs panes are dense with checkboxes;
   a blind focus click toggled "Detect Board Shape" off. If you mis-toggle anything:
   **Cancel discards everything** - nothing persists until Apply/OK (same rule as
   docs/fusion-preferences.md).
6. Scroll panes via the scrollbar channel, not PageDown (Qt panes ignore keyboard paging).

## The one-call future

There is no `fusion_dark_mode` verb yet. Until there is, the sequence is:
`fusion_set_preference {"theme":"darkblue"}`, verify Electronics 2D themes are Dark, then
drive Display Settings > Environment > River Rubicon by clicks and confirm with a
screenshot. If you find yourself doing this twice, wrap it as a verb.
