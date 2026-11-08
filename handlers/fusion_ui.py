"""Win32-based tools for interacting with Fusion 360's UI.

Provides screenshot, click, and keyboard input functions that work with
CEF (Chromium Embedded Framework) modal dialogs inside Fusion 360.
These dialogs can't be accessed via standard Win32 child window enumeration,
so we use screen-DC capture (BitBlt) instead of PrintWindow, and SendInput
instead of PostMessage for input.
"""

import ctypes
import ctypes.wintypes
import os
import struct
import subprocess
import time

# Guard windll so the module imports on non-Windows hosts (the bridge must boot
# + serve /status anywhere); these Win32 paths are only reached on Windows.
_have_windll = hasattr(ctypes, "windll")
user32 = ctypes.windll.user32 if _have_windll else None
gdi32 = ctypes.windll.gdi32 if _have_windll else None
kernel32 = ctypes.windll.kernel32 if _have_windll else None

# ⚠️ HWND TRUNCATION BUG (fixed 2026-07-08, cost hours): with NO argtypes declared, ctypes
# defaults every function arg + return to 32-bit `c_int`, which TRUNCATES 64-bit HWND handles.
# So `_find_fusion_hwnd`'s GetWindowTextW/IsWindowVisible ran on a garbage handle and matched
# NOTHING - get_fusion_window_info returned mainHwnd:None even though Fusion's window ("Untitled
# - Autodesk Fusion") plainly existed, which blinded all background driving. (Verified live: the
# window's real hwnd via `(Get-Process Fusion360).MainWindowHandle` worked fine.) Declaring HWND
# argtypes/restypes makes ctypes marshal the full 64-bit handle. If the finder ever returns 0/None
# while Fusion IS running, the process-handle fallback in _find_fusion_hwnd covers it.
if _have_windll:
    _W = ctypes.wintypes
    for _fn, _arg, _ret in (
        ("IsWindowVisible",       [_W.HWND],                              ctypes.c_bool),
        ("IsWindowEnabled",       [_W.HWND],                              ctypes.c_bool),
        ("GetWindowTextLengthW",  [_W.HWND],                              ctypes.c_int),
        ("GetWindowTextW",        [_W.HWND, _W.LPWSTR, ctypes.c_int],     ctypes.c_int),
        ("GetClassNameW",         [_W.HWND, _W.LPWSTR, ctypes.c_int],     ctypes.c_int),
        ("GetWindow",             [_W.HWND, ctypes.c_uint],               _W.HWND),
        ("GetParent",             [_W.HWND],                              _W.HWND),
        ("IsWindow",              [_W.HWND],                              ctypes.c_bool),
    ):
        try:
            _f = getattr(user32, _fn); _f.argtypes = _arg; _f.restype = _ret
        except Exception:
            pass

# Enable DPI awareness so GetWindowRect returns physical pixel coordinates
# (not logical/scaled). Without this, screenshots are cropped on HiDPI displays.
try:
    ctypes.windll.shcore.SetProcessDpiAwareness(2)  # PROCESS_PER_MONITOR_DPI_AWARE
except Exception:
    try:
        user32.SetProcessDPIAware()  # Fallback for older Windows
    except Exception:
        pass

# --- Constants ---

SRCCOPY = 0x00CC0020
DIB_RGB_COLORS = 0
BI_RGB = 0
SM_CXVIRTUALSCREEN = 78
SM_CYVIRTUALSCREEN = 79

INPUT_MOUSE = 0
INPUT_KEYBOARD = 1
MOUSEEVENTF_ABSOLUTE = 0x8000
MOUSEEVENTF_MOVE = 0x0001
MOUSEEVENTF_LEFTDOWN = 0x0002
MOUSEEVENTF_LEFTUP = 0x0004
KEYEVENTF_KEYUP = 0x0002

# Virtual key codes
VK_MAP = {
    "enter": 0x0D, "return": 0x0D,
    "escape": 0x1B, "esc": 0x1B,
    "tab": 0x09,
    "space": 0x20,
    "up": 0x26, "down": 0x28, "left": 0x25, "right": 0x27,
    "backspace": 0x08, "delete": 0x2E,
    "home": 0x24, "end": 0x23,
    "pageup": 0x21, "pagedown": 0x22,
    "f1": 0x70, "f2": 0x71, "f3": 0x72, "f4": 0x73,
    "f5": 0x74, "f6": 0x75, "f7": 0x76, "f8": 0x77,
    "f9": 0x78, "f10": 0x79, "f11": 0x7A, "f12": 0x7B,
}

SCREENSHOT_DIR = "C:/tmp/conduit-screenshots"

# Keep callback references alive to prevent GC
_callbacks = []


# --- SendInput structures ---

class MOUSEINPUT(ctypes.Structure):
    _fields_ = [
        ("dx", ctypes.wintypes.LONG),
        ("dy", ctypes.wintypes.LONG),
        ("mouseData", ctypes.wintypes.DWORD),
        ("dwFlags", ctypes.wintypes.DWORD),
        ("time", ctypes.wintypes.DWORD),
        ("dwExtraInfo", ctypes.POINTER(ctypes.c_ulong)),
    ]


class KEYBDINPUT(ctypes.Structure):
    _fields_ = [
        ("wVk", ctypes.wintypes.WORD),
        ("wScan", ctypes.wintypes.WORD),
        ("dwFlags", ctypes.wintypes.DWORD),
        ("time", ctypes.wintypes.DWORD),
        ("dwExtraInfo", ctypes.POINTER(ctypes.c_ulong)),
    ]


class _INPUT_UNION(ctypes.Union):
    _fields_ = [
        ("mi", MOUSEINPUT),
        ("ki", KEYBDINPUT),
    ]


class INPUT(ctypes.Structure):
    _fields_ = [
        ("type", ctypes.wintypes.DWORD),
        ("union", _INPUT_UNION),
    ]


def _send_input(*inputs):
    """Send one or more INPUT structures via SendInput."""
    n = len(inputs)
    arr = (INPUT * n)(*inputs)
    user32.SendInput(n, ctypes.pointer(arr), ctypes.sizeof(INPUT))


# --- Window finding ---

def _fusion_window_pids() -> set:
    """PIDs of running Fusion360.exe processes, for title-independent window matching."""
    try:
        out = subprocess.check_output(
            ["tasklist", "/FI", "IMAGENAME eq Fusion360.exe", "/FO", "CSV", "/NH"],
            stdin=subprocess.DEVNULL, stderr=subprocess.DEVNULL, text=True, timeout=5,
            creationflags=(subprocess.CREATE_NO_WINDOW
                           if hasattr(subprocess, "CREATE_NO_WINDOW") else 0))
    except Exception:
        return set()
    pids = set()
    for line in out.splitlines():
        parts = [p.strip('"') for p in line.split('","')]
        if len(parts) > 1:
            try:
                pids.add(int(parts[1]))
            except Exception:
                pass
    return pids


def _find_fusion_hwnd() -> int:
    """Find Fusion 360's MAIN window: the LARGEST visible window OWNED BY A FUSION PROCESS.

    Two generations of this bug, both producing a 237x39 title-bar sliver instead of the
    app:
      1. first-match (fixed 2026-08-15) - Fusion keeps tiny helper/ghost windows carrying
         the same title, and one of them enumerated first.
      2. TITLE-match (fixed 2026-08-19) - picking the largest window *titled* "Autodesk
         Fusion" still fails whenever the main window is mid-transition and does not carry
         that title, leaving the sliver as the only match. The 8/8 surfaces pass caught it:
         the library surface returned a 237x39 capture while every other surface returned
         1568x940.
    Ownership by PID is title-independent, so a retitled or transitioning main window can
    no longer be lost. Title matching stays as the fallback for the case where the process
    list is unreadable.
    """
    pids = _fusion_window_pids()
    if pids:
        best_pid = [0, 0]

        WNDENUMPROC_P = ctypes.WINFUNCTYPE(
            ctypes.c_bool, ctypes.wintypes.HWND, ctypes.wintypes.LPARAM
        )

        def cb_pid(hwnd, _lparam):
            if user32.IsWindowVisible(hwnd):
                pid = ctypes.wintypes.DWORD()
                user32.GetWindowThreadProcessId(hwnd, ctypes.byref(pid))
                if pid.value in pids:
                    try:
                        l, t, r, b = _get_window_rect(hwnd)
                        area = max(0, r - l) * max(0, b - t)
                    except Exception:
                        area = 0
                    if area > best_pid[1]:
                        best_pid[0], best_pid[1] = hwnd, area
            return True

        cbp = WNDENUMPROC_P(cb_pid)
        _callbacks.append(cbp)
        user32.EnumWindows(cbp, 0)
        _callbacks.remove(cbp)
        # Require something PLAUSIBLY the app, not a toast/sliver: anything under
        # 400x300 is not Fusion's main window, so fall through to the title pass.
        if best_pid[0] and best_pid[1] >= 400 * 300:
            return best_pid[0]

    best = [0, 0]  # hwnd, area
    WNDENUMPROC = ctypes.WINFUNCTYPE(
        ctypes.c_bool, ctypes.wintypes.HWND, ctypes.wintypes.LPARAM
    )

    def callback(hwnd, _lparam):
        if user32.IsWindowVisible(hwnd):
            length = user32.GetWindowTextLengthW(hwnd)
            if length > 0:
                buf = ctypes.create_unicode_buffer(length + 1)
                user32.GetWindowTextW(hwnd, buf, length + 1)
                if "Autodesk Fusion" in buf.value:
                    try:
                        l, t, r, b = _get_window_rect(hwnd)
                        area = max(0, r - l) * max(0, b - t)
                    except Exception:
                        area = 0
                    if area > best[1]:
                        best[0], best[1] = hwnd, area
        return True

    cb = WNDENUMPROC(callback)
    _callbacks.append(cb)
    user32.EnumWindows(cb, 0)
    _callbacks.remove(cb)
    return best[0]


def _find_qt_dialog_windows() -> list:
    """Find visible Qt windows that may be Fusion 360 dialogs.

    Returns list of dicts with hwnd, title, rect, className.
    Excludes the main Fusion window (which has 'Autodesk Fusion' in title).
    """
    results = []
    WNDENUMPROC = ctypes.WINFUNCTYPE(
        ctypes.c_bool, ctypes.wintypes.HWND, ctypes.wintypes.LPARAM
    )

    def callback(hwnd, _lparam):
        if user32.IsWindowVisible(hwnd):
            cls_buf = ctypes.create_unicode_buffer(256)
            user32.GetClassNameW(hwnd, cls_buf, 256)
            class_name = cls_buf.value

            length = user32.GetWindowTextLengthW(hwnd)
            title = ""
            if length > 0:
                buf = ctypes.create_unicode_buffer(length + 1)
                user32.GetWindowTextW(hwnd, buf, length + 1)
                title = buf.value

            # Qt windows belonging to Fusion (not the main window)
            if "Qt" in class_name and "Autodesk Fusion" not in title:
                # Some Fusion dialogs (including the "What do you want to
                # design?" startup picker) use Qt Tool window classes like
                # "Qt655QWindowToolSaveBits".  We used to skip ALL Tool
                # windows (they're usually docked panels — Browser, Timeline).
                # But that caused the startup picker to be invisible to
                # dialog detection. Now we only skip SMALL Tool windows
                # (docked panels are narrow sidebar panels, typically
                # <400px wide).  Large Tool windows (>400px wide AND >300px
                # tall) are kept — they're blocking dialogs like the picker.
                if "Tool" in class_name:
                    rect = ctypes.wintypes.RECT()
                    user32.GetWindowRect(hwnd, ctypes.byref(rect))
                    w = rect.right - rect.left
                    h = rect.bottom - rect.top
                    if w < 400 or h < 300:
                        return True  # small tool panel, skip
                    # ASPECT GUARD (2026-07-23): the size test alone is not enough. Fusion's
                    # docked side panels (BROWSER, Timeline, Comments) are TALL and NARROW and
                    # sail past 400x300 - the BROWSER measured 450x1194 and got reported as a
                    # blocking dialog on an idle Fusion, which makes a driving AI believe it is
                    # blocked when nothing is wrong. Real blocking dialogs (the startup picker,
                    # licensing, recovery) are landscape or roughly square. So: anything markedly
                    # taller than it is wide is a docked panel, not a dialog.
                    if h > w * 1.5:
                        return True  # tall narrow docked panel, skip

                rect = ctypes.wintypes.RECT()
                user32.GetWindowRect(hwnd, ctypes.byref(rect))
                results.append({
                    "hwnd": hwnd,
                    "title": title,
                    "className": class_name,
                    "rect": {
                        "left": rect.left, "top": rect.top,
                        "right": rect.right, "bottom": rect.bottom,
                        "width": rect.right - rect.left,
                        "height": rect.bottom - rect.top,
                    },
                })
        return True

    cb = WNDENUMPROC(callback)
    _callbacks.append(cb)
    user32.EnumWindows(cb, 0)
    _callbacks.remove(cb)
    return results


def _get_window_rect(hwnd: int) -> tuple:
    """Get window rect as (left, top, right, bottom)."""
    rect = ctypes.wintypes.RECT()
    user32.GetWindowRect(hwnd, ctypes.byref(rect))
    return (rect.left, rect.top, rect.right, rect.bottom)


# --- Screenshot ---

def _write_bmp(path: str, width: int, height: int, pixel_data: bytes):
    """Write a BMP file from raw BGR pixel data (bottom-up row order).

    pixel_data must be width*height*4 bytes (32-bit BGRA from GetDIBits).
    We write a 24-bit BMP (strip alpha) for broad compatibility.
    """
    row_size_24 = (width * 3 + 3) & ~3  # rows padded to 4 bytes
    pixel_size_24 = row_size_24 * height
    file_size = 14 + 40 + pixel_size_24  # BMP header + DIB header + pixels

    with open(path, "wb") as f:
        # BMP file header (14 bytes)
        f.write(b"BM")
        f.write(struct.pack("<I", file_size))
        f.write(struct.pack("<HH", 0, 0))  # reserved
        f.write(struct.pack("<I", 14 + 40))  # offset to pixel data

        # DIB header (BITMAPINFOHEADER, 40 bytes)
        f.write(struct.pack("<I", 40))  # header size
        f.write(struct.pack("<i", width))
        f.write(struct.pack("<i", height))  # positive = bottom-up
        f.write(struct.pack("<HH", 1, 24))  # planes, bpp
        f.write(struct.pack("<I", BI_RGB))  # compression
        f.write(struct.pack("<I", pixel_size_24))
        f.write(struct.pack("<ii", 2835, 2835))  # pixels/meter (~72 DPI)
        f.write(struct.pack("<II", 0, 0))  # colors

        # Write pixel rows (convert 32-bit BGRA to 24-bit BGR, bottom-up)
        src_row_size = width * 4
        for y in range(height):
            row_start = y * src_row_size
            row_24 = bytearray()
            for x in range(width):
                px = row_start + x * 4
                row_24 += pixel_data[px:px + 3]  # BGR (skip alpha)
            # Pad row to 4-byte boundary
            padding = row_size_24 - len(row_24)
            if padding > 0:
                row_24 += b"\x00" * padding
            f.write(bytes(row_24))


PW_RENDERFULLCONTENT = 0x00000002


def screenshot_hwnd(hwnd: int, label: str = "") -> dict:
    """Capture any window by HWND. Returns {success, savedTo, sizeKB}."""
    # A MINIMIZED window has a degenerate rect and PrintWindow hands back a blank frame -
    # the 870-byte gallery bug (2026-08-15): the launch envelope minimizes Fusion, and every
    # later capture stayed blank. Restore WITHOUT activation, pinned to the BOTTOM of the
    # z-order, so the capture renders while the user's foreground stays untouched.
    try:
        if user32.IsIconic(hwnd):
            user32.ShowWindow(hwnd, 4)  # SW_SHOWNOACTIVATE
            user32.SetWindowPos(hwnd, 1, 0, 0, 0, 0, 0x0001 | 0x0002 | 0x0010)  # HWND_BOTTOM, NOSIZE|NOMOVE|NOACTIVATE
            time.sleep(0.6)  # give Qt a repaint
    except Exception:
        pass
    left, top, right, bottom = _get_window_rect(hwnd)
    width = right - left
    height = bottom - top

    if width <= 0 or height <= 0:
        return {"success": False, "error": f"Invalid rect for hwnd {hwnd}"}

    hdc_screen = user32.GetDC(hwnd)
    hdc_mem = gdi32.CreateCompatibleDC(hdc_screen)
    hbmp = gdi32.CreateCompatibleBitmap(hdc_screen, width, height)
    old_bmp = gdi32.SelectObject(hdc_mem, hbmp)
    user32.PrintWindow(hwnd, hdc_mem, PW_RENDERFULLCONTENT)

    class BITMAPINFOHEADER(ctypes.Structure):
        _fields_ = [
            ("biSize", ctypes.wintypes.DWORD),
            ("biWidth", ctypes.wintypes.LONG),
            ("biHeight", ctypes.wintypes.LONG),
            ("biPlanes", ctypes.wintypes.WORD),
            ("biBitCount", ctypes.wintypes.WORD),
            ("biCompression", ctypes.wintypes.DWORD),
            ("biSizeImage", ctypes.wintypes.DWORD),
            ("biXPelsPerMeter", ctypes.wintypes.LONG),
            ("biYPelsPerMeter", ctypes.wintypes.LONG),
            ("biClrUsed", ctypes.wintypes.DWORD),
            ("biClrImportant", ctypes.wintypes.DWORD),
        ]

    bmi = BITMAPINFOHEADER()
    bmi.biSize = ctypes.sizeof(BITMAPINFOHEADER)
    bmi.biWidth = width
    bmi.biHeight = height
    bmi.biPlanes = 1
    bmi.biBitCount = 32
    bmi.biCompression = BI_RGB

    buf_size = width * height * 4
    pixel_buf = ctypes.create_string_buffer(buf_size)
    gdi32.GetDIBits(hdc_mem, hbmp, 0, height, pixel_buf, ctypes.byref(bmi), DIB_RGB_COLORS)

    gdi32.SelectObject(hdc_mem, old_bmp)
    gdi32.DeleteObject(hbmp)
    gdi32.DeleteDC(hdc_mem)
    user32.ReleaseDC(hwnd, hdc_screen)

    os.makedirs(SCREENSHOT_DIR, exist_ok=True)
    timestamp = int(time.time() * 1000)
    suffix = f"-{label}" if label else ""

    try:
        from PIL import Image
        img = Image.frombytes("RGBA", (width, height), pixel_buf.raw, "raw", "BGRA")
        img = img.transpose(Image.FLIP_TOP_BOTTOM)
        MAX_DIM = 1568
        if max(width, height) > MAX_DIM:
            ratio = MAX_DIM / max(width, height)
            img = img.resize((int(width * ratio), int(height * ratio)), Image.LANCZOS)
        try:
            path = os.path.join(SCREENSHOT_DIR, f"fusion{suffix}-{timestamp}.webp")
            img.save(path, "WEBP", lossless=True, quality=100)
        except Exception:
            path = os.path.join(SCREENSHOT_DIR, f"fusion{suffix}-{timestamp}.png")
            img.save(path, "PNG", optimize=True)
    except ImportError:
        path = os.path.join(SCREENSHOT_DIR, f"fusion{suffix}-{timestamp}.bmp")
        _write_bmp(path, width, height, pixel_buf.raw)

    size_kb = os.path.getsize(path) / 1024
    return {
        "success": True,
        "savedTo": path.replace("\\", "/"),
        "sizeKB": round(size_kb, 1),
    }


def screenshot_fusion_window(use_bitblt: bool = False) -> dict:
    """Capture the Fusion 360 window.

    By default uses PrintWindow with PW_RENDERFULLCONTENT (works without
    foreground, captures Qt content but NOT CEF modal overlays).

    If use_bitblt=True, brings window to foreground and uses BitBlt from
    screen DC (captures CEF overlays but requires foreground).

    Saves PNG to C:/tmp/conduit-screenshots/ and returns the path.
    """
    hwnd = _find_fusion_hwnd()
    if not hwnd:
        return {
            "success": False,
            "error": "Fusion 360 window not found (no window with 'Autodesk Fusion' in title).",
            "_hint": "Call fusion_start to start Fusion 360 and wait for the add-in to be ready, then retry.",
        }

    # Get window rect
    left, top, right, bottom = _get_window_rect(hwnd)
    width = right - left
    height = bottom - top

    if width <= 0 or height <= 0:
        return {"success": False, "error": f"Invalid window rect: {left},{top},{right},{bottom}"}

    # A SLIVER IS NOT EVIDENCE (2026-08-19). If the only window we could find is
    # smaller than the app could possibly be, the main window is mid-transition -
    # wait briefly and re-resolve rather than returning a 237x39 title-bar strip
    # that a caller will happily render as "the library surface, captured".
    if width < 400 or height < 300:
        time.sleep(2.5)
        hwnd2 = _find_fusion_hwnd()
        if hwnd2:
            l2, t2, r2, b2 = _get_window_rect(hwnd2)
            if (r2 - l2) >= 400 and (b2 - t2) >= 300:
                hwnd, left, top, right, bottom = hwnd2, l2, t2, r2, b2
                width, height = right - left, bottom - top

    if use_bitblt:
        # Screen DC capture — needs foreground
        SW_RESTORE = 9
        _prev_fg = _snapshot_foreground()
        user32.ShowWindow(hwnd, SW_RESTORE)
        user32.SetForegroundWindow(hwnd)
        time.sleep(1.0)

        hdc_screen = user32.GetDC(0)
        hdc_mem = gdi32.CreateCompatibleDC(hdc_screen)
        hbmp = gdi32.CreateCompatibleBitmap(hdc_screen, width, height)
        old_bmp = gdi32.SelectObject(hdc_mem, hbmp)
        gdi32.BitBlt(hdc_mem, 0, 0, width, height, hdc_screen, left, top, SRCCOPY)
        _handback_foreground(_prev_fg, hwnd)
    else:
        # PrintWindow capture — works without foreground
        hdc_screen = user32.GetDC(hwnd)
        hdc_mem = gdi32.CreateCompatibleDC(hdc_screen)
        hbmp = gdi32.CreateCompatibleBitmap(hdc_screen, width, height)
        old_bmp = gdi32.SelectObject(hdc_mem, hbmp)
        user32.PrintWindow(hwnd, hdc_mem, PW_RENDERFULLCONTENT)

    # Read pixel data via GetDIBits
    class BITMAPINFOHEADER(ctypes.Structure):
        _fields_ = [
            ("biSize", ctypes.wintypes.DWORD),
            ("biWidth", ctypes.wintypes.LONG),
            ("biHeight", ctypes.wintypes.LONG),
            ("biPlanes", ctypes.wintypes.WORD),
            ("biBitCount", ctypes.wintypes.WORD),
            ("biCompression", ctypes.wintypes.DWORD),
            ("biSizeImage", ctypes.wintypes.DWORD),
            ("biXPelsPerMeter", ctypes.wintypes.LONG),
            ("biYPelsPerMeter", ctypes.wintypes.LONG),
            ("biClrUsed", ctypes.wintypes.DWORD),
            ("biClrImportant", ctypes.wintypes.DWORD),
        ]

    bmi = BITMAPINFOHEADER()
    bmi.biSize = ctypes.sizeof(BITMAPINFOHEADER)
    bmi.biWidth = width
    bmi.biHeight = height  # positive = bottom-up (standard BMP order)
    bmi.biPlanes = 1
    bmi.biBitCount = 32
    bmi.biCompression = BI_RGB

    buf_size = width * height * 4
    pixel_buf = ctypes.create_string_buffer(buf_size)
    gdi32.GetDIBits(hdc_mem, hbmp, 0, height, pixel_buf, ctypes.byref(bmi), DIB_RGB_COLORS)

    # Clean up GDI resources
    gdi32.SelectObject(hdc_mem, old_bmp)
    gdi32.DeleteObject(hbmp)
    gdi32.DeleteDC(hdc_mem)
    if use_bitblt:
        user32.ReleaseDC(0, hdc_screen)
    else:
        user32.ReleaseDC(hwnd, hdc_screen)

    # Save to file as PNG (try PIL first, fall back to BMP)
    os.makedirs(SCREENSHOT_DIR, exist_ok=True)
    timestamp = int(time.time() * 1000)

    try:
        from PIL import Image
        # pixel_buf is BGRA bottom-up; convert to RGBA top-down
        img = Image.frombytes("RGBA", (width, height), pixel_buf.raw, "raw", "BGRA")
        img = img.transpose(Image.FLIP_TOP_BOTTOM)

        # Downscale to max 1568px on longest side (saves tokens for AI vision)
        MAX_DIM = 1568
        if max(width, height) > MAX_DIM:
            ratio = MAX_DIM / max(width, height)
            new_size = (int(width * ratio), int(height * ratio))
            img = img.resize(new_size, Image.LANCZOS)

        # Try WebP first (much smaller than PNG for screenshots)
        try:
            path = os.path.join(SCREENSHOT_DIR, f"fusion-{timestamp}.webp")
            img.save(path, "WEBP", lossless=True, quality=100)
        except Exception:
            # Fall back to PNG if WebP not available
            path = os.path.join(SCREENSHOT_DIR, f"fusion-{timestamp}.png")
            img.save(path, "PNG", optimize=True)
    except ImportError:
        # No PIL — save as BMP (readable by most tools, but not Claude Read)
        path = os.path.join(SCREENSHOT_DIR, f"fusion-{timestamp}.bmp")
        _write_bmp(path, width, height, pixel_buf.raw)

    size_kb = os.path.getsize(path) / 1024
    return {
        "success": True,
        "savedTo": path.replace("\\", "/"),
        "sizeKB": round(size_kb, 1),
        "dimensions": {"width": width, "height": height},
    }


# --- Click ---

def _snapshot_foreground():
    """The USER'S current foreground window, so a focus-requiring action can hand it back."""
    try:
        return user32.GetForegroundWindow()
    except Exception:
        return 0


def _handback_foreground(prev, taken_hwnd):
    """Give the foreground BACK after a focus-requiring action (John, 2026-08-15: taking
    fg for an ESC and never restoring is as rude as the steal). No-op if the user was
    already in the window we took."""
    try:
        if prev and prev != taken_hwnd and user32.IsWindow(prev):
            time.sleep(0.15)
            user32.SetForegroundWindow(prev)
    except Exception:
        pass


def click_fusion(x, y, relative=True, hwnd=None) -> dict:
    """Click at coordinates in a window (Fusion main window or any dialog).

    Args:
        x: X coordinate. If relative=True, a float 0.0-1.0 (percentage of window width).
           If relative=False, pixel offset from window's top-left corner.
        y: Y coordinate. Same convention as x but for height.
        relative: If True (default), x/y are percentages. If False, pixel offsets.
        hwnd: Optional HWND to click on. If omitted, finds the main Fusion window.
              Use this to click buttons inside Qt dialogs by passing the dialog's HWND.

    Uses SendInput (NOT PostMessage) because CEF doesn't respond to WM messages.
    """
    if hwnd:
        hwnd = int(hwnd)
        # Validate the HWND is a real window
        if not user32.IsWindow(hwnd):
            return {
                "success": False,
                "error": f"HWND {hwnd} is not a valid window.",
                "_hint": "HWNDs change when windows close/reopen. Call fusion_get_window_info to get fresh HWNDs, then retry.",
            }
    else:
        hwnd = _find_fusion_hwnd()
        if not hwnd:
            return {
                "success": False,
                "error": "Fusion 360 window not found.",
                "_hint": "Call fusion_start to start Fusion 360, then retry.",
            }

    left, top, right, bottom = _get_window_rect(hwnd)
    width = right - left
    height = bottom - top

    # Bring to front - snapshot the user's window first, hand it back after the click
    _prev_fg = _snapshot_foreground()
    user32.SetForegroundWindow(hwnd)
    time.sleep(0.1)

    # Calculate screen coordinates
    if relative:
        screen_x = left + int(float(x) * width)
        screen_y = top + int(float(y) * height)
    else:
        screen_x = left + int(x)
        screen_y = top + int(y)

    # Convert to absolute coordinates for SendInput (0-65535 range)
    screen_w = user32.GetSystemMetrics(0)  # SM_CXSCREEN (primary monitor)
    screen_h = user32.GetSystemMetrics(1)  # SM_CYSCREEN
    abs_x = int(screen_x * 65535 / screen_w)
    abs_y = int(screen_y * 65535 / screen_h)

    # Move mouse
    move = INPUT()
    move.type = INPUT_MOUSE
    move.union.mi.dx = abs_x
    move.union.mi.dy = abs_y
    move.union.mi.dwFlags = MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_MOVE

    # Click down
    down = INPUT()
    down.type = INPUT_MOUSE
    down.union.mi.dx = abs_x
    down.union.mi.dy = abs_y
    down.union.mi.dwFlags = MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_MOVE | MOUSEEVENTF_LEFTDOWN

    # Click up
    up = INPUT()
    up.type = INPUT_MOUSE
    up.union.mi.dx = abs_x
    up.union.mi.dy = abs_y
    up.union.mi.dwFlags = MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_MOVE | MOUSEEVENTF_LEFTUP

    _send_input(move, down, up)
    _handback_foreground(_prev_fg, hwnd)

    return {
        "success": True,
        "clickedAt": {"screenX": screen_x, "screenY": screen_y},
        "windowOffset": {
            "x": screen_x - left,
            "y": screen_y - top,
        },
        "relative": relative,
    }


# --- Keyboard ---

def send_key_background(key: str, hwnd=None) -> dict:
    """Send a key via PostMessage - NO foreground, no focus steal, the KiCad-bridge
    lesson (John, 2026-08-15: kicad reads real menus + PostCommand instead of
    foreground+SendInput and 'now kicad works better'). Works for the dialog keys that
    matter (escape/enter/tab) on Win32 and most Qt dialogs; SendInput stays as the
    explicit, captioned last resort for CEF surfaces that ignore posted input.
    """
    if hwnd:
        hwnd = int(hwnd)
        if not user32.IsWindow(hwnd):
            return {"success": False, "error": f"HWND {hwnd} is not a valid window."}
    else:
        hwnd = _find_fusion_hwnd()
        if not hwnd:
            return {"success": False, "error": "Fusion 360 window not found."}
    vk = VK_MAP.get(key.lower().strip())
    if vk is None and len(key) == 1:
        vk = user32.VkKeyScanW(ord(key)) & 0xFF
    if vk in (None, 0xFF):
        return {"success": False, "error": f"Cannot map key '{key}' for the background path."}
    WM_KEYDOWN, WM_KEYUP = 0x0100, 0x0101
    ok1 = user32.PostMessageW(hwnd, WM_KEYDOWN, vk, 0x00000001)
    ok2 = user32.PostMessageW(hwnd, WM_KEYUP, vk, 0xC0000001)
    if not (ok1 and ok2):
        return {"success": False, "error": "PostMessage refused the key (window gone or access denied)."}
    return {"success": True, "key": key, "vk": vk, "method": "postmessage-background"}


def send_key_to_fusion(key: str, hwnd=None) -> dict:
    """Send a key to Fusion 360 (or a specific dialog) via SendInput.

    Args:
        key: One of: "enter", "escape", "tab", "space", "up", "down",
             "left", "right", "f1"-"f12", "backspace", "delete",
             "home", "end", "pageup", "pagedown", or a single character.
        hwnd: Optional HWND to target. If omitted, targets the main Fusion window.
    """
    if hwnd:
        hwnd = int(hwnd)
        if not user32.IsWindow(hwnd):
            return {
                "success": False,
                "error": f"HWND {hwnd} is not a valid window.",
                "_hint": "HWNDs change when windows close/reopen. Call fusion_get_window_info to get fresh HWNDs, then retry.",
            }
    else:
        hwnd = _find_fusion_hwnd()
        if not hwnd:
            return {
                "success": False,
                "error": "Fusion 360 window not found.",
                "_hint": "Call fusion_start to start Fusion 360, then retry.",
            }

    # Bring to front - snapshot the user's window first, hand it back after the key lands
    _prev_fg = _snapshot_foreground()
    user32.SetForegroundWindow(hwnd)
    time.sleep(0.1)

    key_lower = key.lower().strip()
    vk = VK_MAP.get(key_lower)

    if vk is None:
        if len(key) == 1:
            # Single character: use VkKeyScanW to get the virtual key code
            vk_scan = user32.VkKeyScanW(ord(key))
            vk = vk_scan & 0xFF
            shift = (vk_scan >> 8) & 0x01
            if vk == 0xFF:
                return {"success": False, "error": f"Cannot map character '{key}' to a virtual key."}

            inputs = []
            # Press shift if needed
            if shift:
                s_down = INPUT()
                s_down.type = INPUT_KEYBOARD
                s_down.union.ki.wVk = 0x10  # VK_SHIFT
                inputs.append(s_down)

            # Key down
            kd = INPUT()
            kd.type = INPUT_KEYBOARD
            kd.union.ki.wVk = vk
            inputs.append(kd)

            # Key up
            ku = INPUT()
            ku.type = INPUT_KEYBOARD
            ku.union.ki.wVk = vk
            ku.union.ki.dwFlags = KEYEVENTF_KEYUP
            inputs.append(ku)

            # Release shift if needed
            if shift:
                s_up = INPUT()
                s_up.type = INPUT_KEYBOARD
                s_up.union.ki.wVk = 0x10
                s_up.union.ki.dwFlags = KEYEVENTF_KEYUP
                inputs.append(s_up)

            _send_input(*inputs)
            _handback_foreground(_prev_fg, hwnd)
            return {"success": True, "key": key, "vk": vk, "shift": bool(shift)}
        else:
            return {"success": False, "error": f"Unknown key: '{key}'. Use a named key or single character."}

    # Named key: simple down + up
    kd = INPUT()
    kd.type = INPUT_KEYBOARD
    kd.union.ki.wVk = vk

    ku = INPUT()
    ku.type = INPUT_KEYBOARD
    ku.union.ki.wVk = vk
    ku.union.ki.dwFlags = KEYEVENTF_KEYUP

    _send_input(kd, ku)
    _handback_foreground(_prev_fg, hwnd)
    return {"success": True, "key": key_lower, "vk": vk}


def close_window(hwnd: int) -> dict:
    """Close a window by sending WM_CLOSE via PostMessage.

    Unlike Escape (which many Fusion dialogs ignore — e.g. Recovered Documents),
    WM_CLOSE is the standard Windows mechanism for closing a window and is
    handled by virtually all Qt dialogs.

    This does NOT force-kill the window — the dialog can still prompt for
    confirmation. It's equivalent to clicking the X button in the title bar.

    Args:
        hwnd: HWND of the window to close.
    """
    hwnd = int(hwnd)
    if not user32.IsWindow(hwnd):
        return {
            "success": False,
            "error": f"HWND {hwnd} is not a valid window.",
        }

    WM_CLOSE = 0x0010
    user32.PostMessageW(hwnd, WM_CLOSE, 0, 0)
    return {"success": True, "hwnd": hwnd, "action": "WM_CLOSE sent"}


# --- Window info ---

def get_fusion_window_info() -> dict:
    """Get Fusion 360 window info: hwnd, title, rect, and any Qt dialog windows.

    Returns the main Fusion window details plus a list of other Qt windows
    that may be Fusion dialogs (file pickers, recovery prompts, etc.).
    """
    hwnd = _find_fusion_hwnd()
    if not hwnd:
        return {
            "success": False,
            "error": "Fusion 360 window not found.",
            "_hint": "Call fusion_start to start Fusion 360, then retry.",
        }

    # Get title
    length = user32.GetWindowTextLengthW(hwnd)
    title = ""
    if length > 0:
        buf = ctypes.create_unicode_buffer(length + 1)
        user32.GetWindowTextW(hwnd, buf, length + 1)
        title = buf.value

    left, top, right, bottom = _get_window_rect(hwnd)

    # Find potential dialog windows
    dialogs = _find_qt_dialog_windows()

    # A real modal dialog DISABLES its owner window; docked panels (Browser,
    # Timeline) leave it ENABLED. This is the clean signal that separates an
    # actual blocking modal from a tool window that merely shares the "Fusion360"
    # title and slips past the size filter. Callers use it to suppress
    # false-positive "dialogs" when nothing is actually blocking.
    try:
        main_enabled = bool(user32.IsWindowEnabled(hwnd))
    except Exception:
        main_enabled = True

    return {
        "success": True,
        "hwnd": hwnd,
        "title": title,
        "mainEnabled": main_enabled,
        "rect": {
            "left": left, "top": top,
            "right": right, "bottom": bottom,
            "width": right - left,
            "height": bottom - top,
        },
        "dialogs": dialogs,
    }

# ── Window SIZING (John, 2026-08-18: "why is your fusion window always kind of small and not
# like 90% of the screen size?") ─────────────────────────────────────────────────────────────
# Fusion opens at whatever geometry it last had, which after a headless/automated launch is
# often a small default - fine for the bridge (PrintWindow captures any size) but lousy for the
# human who then looks at it, and it wastes screenshot resolution for the AI too. So after a
# launch we grow the window to ~92% of the monitor's WORK AREA, centered.
#
# CRITICAL: this must not steal focus. SetWindowPos with SWP_NOACTIVATE|SWP_NOZORDER resizes a
# window WITHOUT activating it or changing z-order - unlike ShowWindow(SW_MAXIMIZE), which
# activates and would violate the background etiquette. We also never touch a window the user
# already maximized or already sized large (>=60% of the work area) - that is their choice.

class _MONITORINFO(ctypes.Structure):
    _fields_ = [("cbSize", ctypes.wintypes.DWORD),
                ("rcMonitor", ctypes.wintypes.RECT),
                ("rcWork", ctypes.wintypes.RECT),
                ("dwFlags", ctypes.wintypes.DWORD)]


def ensure_reasonable_window(fraction: float = 0.92, hwnd=None) -> dict:
    """Grow a small Fusion window to `fraction` of its monitor's work area, centered.
    Background-safe (no activation, no z-order change). Never raises."""
    try:
        user32 = ctypes.windll.user32
        hwnd = int(hwnd) if hwnd else _find_fusion_hwnd()
        if not hwnd:
            return {"sized": False, "why": "no fusion window"}
        if user32.IsZoomed(hwnd):
            return {"sized": False, "why": "already maximized (user's choice)"}
        mon = user32.MonitorFromWindow(hwnd, 2)          # MONITOR_DEFAULTTONEAREST
        mi = _MONITORINFO()
        mi.cbSize = ctypes.sizeof(_MONITORINFO)
        if not user32.GetMonitorInfoW(mon, ctypes.byref(mi)):
            return {"sized": False, "why": "GetMonitorInfo failed"}
        wa_w = mi.rcWork.right - mi.rcWork.left
        wa_h = mi.rcWork.bottom - mi.rcWork.top
        left, top, right, bottom = _get_window_rect(hwnd)
        cur_w, cur_h = right - left, bottom - top
        if cur_w * cur_h >= 0.60 * wa_w * wa_h:
            return {"sized": False, "why": "already large enough",
                    "current": [cur_w, cur_h], "workArea": [wa_w, wa_h]}
        new_w, new_h = int(wa_w * fraction), int(wa_h * fraction)
        x = mi.rcWork.left + (wa_w - new_w) // 2
        y = mi.rcWork.top + (wa_h - new_h) // 2
        SWP_NOZORDER, SWP_NOACTIVATE, SWP_NOOWNERZORDER = 0x0004, 0x0010, 0x0200
        ok = bool(user32.SetWindowPos(hwnd, 0, x, y, new_w, new_h,
                                      SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOOWNERZORDER))
        return {"sized": ok, "from": [cur_w, cur_h], "to": [new_w, new_h],
                "workArea": [wa_w, wa_h], "hwnd": hwnd,
                "why": "grew to %d%% of the work area without activating it" % int(fraction * 100)}
    except Exception as e:
        return {"sized": False, "why": "error: %s" % str(e)[:120]}


def is_fusion_hung() -> dict:
    """Is the Fusion HOST PROCESS hung, as opposed to the add-in being down?

    They need OPPOSITE responses from a caller (page issue #38): an add-in that is down
    can be reloaded by restarting Fusion politely; a process that has stopped pumping its
    message queue cannot be reached over IPC at all, so nothing but a forced restart helps.
    Reporting the second as the first sent a reader hunting a blocking dialog that did not
    exist.

    Two signals, cheap and independent:
      - IsHungAppWindow: the OS's own answer, which is what the shell uses.
      - a top-level window of class "Ghost": the placeholder Windows substitutes for a
        non-pumping window, so its presence is corroboration that survives a racy probe.
    Returns {hung, signals[], hwnd}. Never raises - a detector that can throw is worse
    than no detector.
    """
    out = {"hung": False, "signals": [], "hwnd": None}
    try:
        hwnd = _find_fusion_hwnd()
        out["hwnd"] = hwnd or None
        if hwnd:
            try:
                if bool(user32.IsHungAppWindow(hwnd)):
                    out["hung"] = True
                    out["signals"].append("IsHungAppWindow")
            except Exception:
                pass
        # Ghost-class corroboration: enumerate top-level windows and look at the class.
        ghosts = []
        WNDENUMPROC_G = ctypes.WINFUNCTYPE(ctypes.c_bool, ctypes.wintypes.HWND,
                                           ctypes.wintypes.LPARAM)

        def cb(h, _l):
            try:
                buf = ctypes.create_unicode_buffer(64)
                user32.GetClassNameW(h, buf, 64)
                if buf.value == "Ghost" and user32.IsWindowVisible(h):
                    t = ctypes.create_unicode_buffer(256)
                    user32.GetWindowTextW(h, t, 256)
                    if "Fusion" in (t.value or ""):
                        ghosts.append(h)
            except Exception:
                pass
            return True

        c = WNDENUMPROC_G(cb)
        _callbacks.append(c)
        user32.EnumWindows(c, 0)
        _callbacks.remove(c)
        if ghosts:
            out["hung"] = True
            out["signals"].append("GhostWindow")
            out["ghostHwnds"] = ghosts
    except Exception:
        pass
    return out
