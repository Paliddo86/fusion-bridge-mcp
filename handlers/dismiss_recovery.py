"""Dismiss Fusion 360 recovery dialogs via Win32 API.

After a crash or force-kill, Fusion shows two types of recovery dialogs:

1. "Recovered Documents" list dialog (Qt window titled "Fusion360"):
   Shows all recovery files with Open/Delete/Close buttons.
   Dismissed via WM_CLOSE.

2. "Open recovery document instead?" prompt (Qt window with that title):
   Appears when opening a file that has a recovery document.
   Has Yes/No buttons. We click No (Tab + Enter) to skip recovery
   and open the cloud version instead.

Both are Qt-rendered dialogs with no Win32 child controls, so we use
keyboard navigation (Tab/Enter) instead of button clicking.

Recovery files are NOT deleted — they are moved to ~/.adom/recovery/fusion/
so the user can manually recover them if needed. Each batch is timestamped.
"""

import ctypes
import ctypes.wintypes
import glob
import os
import shutil
import time

# Guard windll so the module imports on non-Windows hosts (the bridge must boot
# + serve /status anywhere); these Win32 paths are only reached on Windows.
user32 = ctypes.windll.user32 if hasattr(ctypes, "windll") else None

WM_CLOSE = 0x0010
WM_COMMAND = 0x0111
BM_CLICK = 0x00F5

# Keep callback references alive to prevent GC
_callbacks = []


def _get_fusion_crash_recovery_dirs() -> list:
    """Find all CrashRecovery directories under Fusion's local app data.

    Fusion stores recovery files at:
      %LOCALAPPDATA%/Autodesk/Autodesk Fusion 360/<USER_ID>/CrashRecovery/

    There may be multiple user ID folders. Returns list of existing
    CrashRecovery dirs that contain at least one file.
    """
    base = os.path.join(
        os.environ.get("LOCALAPPDATA", ""),
        "Autodesk", "Autodesk Fusion 360"
    )
    if not os.path.isdir(base):
        return []

    dirs = []
    for entry in os.listdir(base):
        cr_dir = os.path.join(base, entry, "CrashRecovery")
        if os.path.isdir(cr_dir) and os.listdir(cr_dir):
            dirs.append(cr_dir)
    return dirs


def relocate_recovery_files() -> dict:
    """Move Fusion crash recovery files to ~/.adom/recovery/fusion/.

    Instead of deleting recovery files (which removes a useful safety net),
    we relocate them to Adom's own recovery folder. Each batch gets a
    timestamped subfolder so the user can find and restore them if needed.

    Returns dict with:
      - moved: number of files/folders moved
      - dest: destination directory (or None if nothing moved)
      - sources: list of source CrashRecovery dirs that were processed
    """
    cr_dirs = _get_fusion_crash_recovery_dirs()
    if not cr_dirs:
        return {"moved": 0, "dest": None, "sources": []}

    # Create timestamped destination
    timestamp = time.strftime("%Y%m%d_%H%M%S")
    adom_home = os.path.join(os.path.expanduser("~"), ".adom")
    dest_base = os.path.join(adom_home, "recovery", "fusion", timestamp)
    os.makedirs(dest_base, exist_ok=True)

    moved = 0
    sources = []
    for cr_dir in cr_dirs:
        sources.append(cr_dir)
        for item in os.listdir(cr_dir):
            src = os.path.join(cr_dir, item)
            dst = os.path.join(dest_base, item)
            # Handle name collisions by appending a suffix
            if os.path.exists(dst):
                base_name, ext = os.path.splitext(item)
                dst = os.path.join(dest_base, f"{base_name}_{moved}{ext}")
            try:
                shutil.move(src, dst)
                moved += 1
            except Exception:
                # If move fails (file locked), try copy-then-delete
                try:
                    if os.path.isdir(src):
                        shutil.copytree(src, dst)
                    else:
                        shutil.copy2(src, dst)
                    shutil.rmtree(src) if os.path.isdir(src) else os.remove(src)
                    moved += 1
                except Exception:
                    pass  # File may be locked by Fusion — skip it

    # Clean up empty CrashRecovery dirs
    for cr_dir in cr_dirs:
        try:
            if os.path.isdir(cr_dir) and not os.listdir(cr_dir):
                os.rmdir(cr_dir)
        except Exception:
            pass

    # If nothing was actually moved, remove the empty timestamp dir
    if moved == 0:
        try:
            os.rmdir(dest_base)
        except Exception:
            pass
        return {"moved": 0, "dest": None, "sources": sources}

    return {"moved": moved, "dest": dest_base, "sources": sources}


def _find_window_by_title(title_substring: str) -> int:
    """Find a visible window whose title contains the given substring."""
    result = [0]
    WNDENUMPROC = ctypes.WINFUNCTYPE(
        ctypes.c_bool, ctypes.wintypes.HWND, ctypes.wintypes.LPARAM
    )

    def callback(hwnd, _lparam):
        if user32.IsWindowVisible(hwnd):
            length = user32.GetWindowTextLengthW(hwnd)
            if length > 0:
                buf = ctypes.create_unicode_buffer(length + 1)
                user32.GetWindowTextW(hwnd, buf, length + 1)
                if title_substring in buf.value:
                    result[0] = hwnd
                    return False  # Stop enumeration
        return True

    cb = WNDENUMPROC(callback)
    _callbacks.append(cb)
    user32.EnumWindows(cb, 0)
    _callbacks.remove(cb)
    return result[0]


def _find_fusion_dialog_windows() -> list:
    """Find small Qt dialog windows that belong to Fusion 360.

    Fusion's recovery dialog is a Qt window with class 'Qt655QWindowIcon'
    and title 'Fusion360' (NOT 'Recovered Documents'). It's smaller than
    the main Fusion window. We detect it by finding Qt windows titled
    'Fusion360' that are NOT the main Fusion window (which has
    'Autodesk Fusion' in its title).
    """
    results = []
    _matched, _skipped_big = [], []
    WNDENUMPROC = ctypes.WINFUNCTYPE(
        ctypes.c_bool, ctypes.wintypes.HWND, ctypes.wintypes.LPARAM
    )

    def callback(hwnd, _lparam):
        if user32.IsWindowVisible(hwnd):
            # Get class name
            cls_buf = ctypes.create_unicode_buffer(256)
            user32.GetClassNameW(hwnd, cls_buf, 256)
            class_name = cls_buf.value

            # Get title
            length = user32.GetWindowTextLengthW(hwnd)
            title = ""
            if length > 0:
                buf = ctypes.create_unicode_buffer(length + 1)
                user32.GetWindowTextW(hwnd, buf, length + 1)
                title = buf.value

            # Look for Qt DIALOG windows titled "Fusion360" (not the main window).
            # ⛔ CLASS GATE (#50 ROOT CAUSE, ledger-proven 2026-08-23 14:41): Fusion's own
            # TOOL windows (class Qt6xxQWindowToolSaveBits - toolbar/panel hosts) are ALSO
            # titled exactly "Fusion360", and a WM_CLOSE to one of them EXITS FUSION: a
            # clean exit, no crash artifact. The janitor's 300s sweep matched them on
            # `"Qt" in class_name` and killed every live Fusion within five minutes -
            # the +344s mid-prove death, 9/9 chains, the 5/5 pinned-1.9.183 stragglers.
            # The recovery list is a top-level DIALOG: class QWindowIcon. Nothing else.
            if "QWindowIcon" in class_name and "ToolSaveBits" not in class_name and title == "Fusion360":
                # SIZE GATE (#50, 2026-08-23): the recovery LIST is a small dialog. A
                # window this title+class but main-window sized is Fusion's product frame
                # (seen with zero documents open, i.e. right after a chain closes them) -
                # WM_CLOSE to that is a clean, artifact-free Fusion EXIT. Never close it.
                r = ctypes.wintypes.RECT()
                user32.GetWindowRect(hwnd, ctypes.byref(r))
                w, h = r.right - r.left, r.bottom - r.top
                if w >= 700 or h >= 500:
                    _skipped_big.append({"hwnd": hwnd, "cls": class_name, "w": w, "h": h})
                else:
                    results.append(hwnd)
                    _matched.append({"hwnd": hwnd, "cls": class_name, "w": w, "h": h})
        return True

    cb = WNDENUMPROC(callback)
    _callbacks.append(cb)
    user32.EnumWindows(cb, 0)
    _callbacks.remove(cb)
    LAST_SCAN["matched"] = _matched
    LAST_SCAN["skippedBig"] = _skipped_big
    return results


LAST_SCAN = {"matched": [], "skippedBig": []}   # what the last "Fusion360" scan saw
LEDGER = None   # server.py installs its _bridge_action_ledger here so closes are recorded


def _ledger(action, detail):
    try:
        if LEDGER:
            LEDGER(action, detail)
    except Exception:
        pass


def _find_child_button(parent_hwnd: int, button_text: str) -> int:
    """Find a child button control with the given text."""
    result = [0]
    WNDENUMPROC = ctypes.WINFUNCTYPE(
        ctypes.c_bool, ctypes.wintypes.HWND, ctypes.wintypes.LPARAM
    )

    def callback(hwnd, _lparam):
        # Get the control text
        length = user32.GetWindowTextLengthW(hwnd)
        if length > 0:
            buf = ctypes.create_unicode_buffer(length + 1)
            user32.GetWindowTextW(hwnd, buf, length + 1)
            if buf.value.strip().lower() == button_text.lower():
                result[0] = hwnd
                return False
        return True

    cb = WNDENUMPROC(callback)
    _callbacks.append(cb)
    user32.EnumChildWindows(parent_hwnd, cb, 0)
    _callbacks.remove(cb)
    return result[0]


VK_ESCAPE = 0x1B
VK_TAB = 0x09
VK_RETURN = 0x0D
WM_KEYDOWN = 0x0100
WM_KEYUP = 0x0101


def _send_key(hwnd: int, vk: int, pause: float = 0.05):
    """Send a key press (down + up) to a window."""
    user32.PostMessageW(hwnd, WM_KEYDOWN, vk, 0)
    time.sleep(pause)
    user32.PostMessageW(hwnd, WM_KEYUP, vk, 0)


def _dismiss_recovery_prompt(hwnd: int) -> bool:
    """Dismiss an 'Open recovery document instead?' Qt dialog by clicking No.

    The dialog has Yes (focused by default) and No buttons.
    Tab moves focus to No, Enter clicks it. This skips the recovery
    and opens the cloud version instead.
    """
    user32.SetForegroundWindow(hwnd)
    time.sleep(0.2)
    _send_key(hwnd, VK_TAB)
    time.sleep(0.2)
    _send_key(hwnd, VK_RETURN)
    return True


def dismiss_recovery_dialog(max_wait: float = 5.0, background: bool = False) -> bool:
    """Find and dismiss all Fusion 360 recovery dialogs.

    Before dismissing, relocates recovery files from Fusion's CrashRecovery
    directory to ~/.adom/recovery/fusion/<timestamp>/ so the user can
    manually recover them if needed.

    Handles two dialog types:
    1. "Open recovery document instead?" — Qt dialog, dismissed via Tab+Enter (No)
    2. "Fusion360" titled Qt dialog — the Recovered Documents list, dismissed via WM_CLOSE
    3. Fallback: Send Escape to Fusion main window

    Loops to handle multiple recovery prompts (one per recovered file).
    Returns True if any dismiss attempt was made.
    """
    # Relocate recovery files BEFORE dismissing dialogs
    # This preserves them in ~/.adom/recovery/fusion/ for the user
    had_recovery_files = False
    try:
        reloc = relocate_recovery_files()
        had_recovery_files = reloc.get("moved", 0) > 0
        if reloc["moved"] > 0:
            import logging
            logging.getLogger(__name__).info(
                "Relocated %d recovery file(s) to %s",
                reloc["moved"], reloc["dest"]
            )
    except Exception:
        pass  # Don't let relocation failure block dialog dismissal

    deadline = time.monotonic() + max_wait
    dismissed_any = False

    while time.monotonic() < deadline:
        # Priority 1: "Open recovery document instead?" prompt
        prompt_hwnd = _find_window_by_title("Open recovery document instead?")
        if prompt_hwnd:
            _dismiss_recovery_prompt(prompt_hwnd)
            dismissed_any = True
            time.sleep(0.5)
            continue  # Check for more prompts

        # Priority 2: "Fusion360" titled Qt dialogs (Recovered Documents list)
        dialog_hwnds = _find_fusion_dialog_windows()
        if LAST_SCAN.get("skippedBig"):
            _ledger("recovery_scan_skipped_main_sized", {"windows": LAST_SCAN["skippedBig"]})
        if dialog_hwnds:
            for hwnd in dialog_hwnds:
                user32.PostMessageW(hwnd, WM_CLOSE, 0, 0)
            _ledger("recovery_wm_close", {"windows": LAST_SCAN.get("matched")})
            dismissed_any = True
            time.sleep(0.5)
            continue  # Check for more

        # Priority 3: Look for a window explicitly titled "Recovered Documents"
        hwnd = _find_window_by_title("Recovered Documents")
        if hwnd:
            close_btn = _find_child_button(hwnd, "Close")
            if close_btn:
                user32.PostMessageW(close_btn, BM_CLICK, 0, 0)
            else:
                user32.PostMessageW(hwnd, WM_CLOSE, 0, 0)
            dismissed_any = True
            time.sleep(0.5)
            continue

        # If we already dismissed something, check one more time
        if dismissed_any:
            time.sleep(0.5)
            # One final check
            if not _find_window_by_title("Open recovery document instead?") \
               and not _find_fusion_dialog_windows() \
               and not _find_window_by_title("Recovered Documents"):
                break
            continue

        # Priority 4: Send Escape to the main Fusion window (last resort).
        # ⛔ LAUNCH-TIME ONLY. This used to fire from the janitor's 300s sweep on EVERY
        # live Fusion when nothing matched - an Escape keystroke into a working Fusion
        # every five minutes, cancelling whatever command was running (lost evidence
        # frames, 'f3d saveAs did not produce a path', #50). In background mode there is
        # no fallback: if no recovery dialog is found, nothing is touched.
        if background:
            break
        # Blind-Escape gate (ledger, 2026-08-23 14:49-50: THREE Escapes into a healthy
        # starting Fusion with no recovery state at all). The last-resort Escape only
        # makes sense when recovery files actually existed this launch - otherwise there
        # is nothing to dismiss and the keystroke can only cancel real work.
        if not had_recovery_files:
            break
        fusion_hwnd = _find_window_by_title("Autodesk Fusion")
        if fusion_hwnd:
            _send_key(fusion_hwnd, VK_ESCAPE)
            _ledger("recovery_escape_fallback", {"hwnd": fusion_hwnd})
            dismissed_any = True
            time.sleep(1.0)
            continue

        time.sleep(0.5)

    return dismissed_any
