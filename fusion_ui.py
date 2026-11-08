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

def _find_fusion_hwnd() -> int:
    """Find Fusion 360's main window by title containing 'Autodesk Fusion'."""
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
                if "Autodesk Fusion" in buf.value:
                    result[0] = hwnd
                    return False
        return True

    cb = WNDENUMPROC(callback)
    _callbacks.append(cb)
    user32.EnumWindows(cb, 0)
    _callbacks.remove(cb)
    return result[0]


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

    if use_bitblt:
        # Screen DC capture — needs foreground
        SW_RESTORE = 9
        user32.ShowWindow(hwnd, SW_RESTORE)
        user32.SetForegroundWindow(hwnd)
        time.sleep(1.0)

        hdc_screen = user32.GetDC(0)
        hdc_mem = gdi32.CreateCompatibleDC(hdc_screen)
        hbmp = gdi32.CreateCompatibleBitmap(hdc_screen, width, height)
        old_bmp = gdi32.SelectObject(hdc_mem, hbmp)
        gdi32.BitBlt(hdc_mem, 0, 0, width, height, hdc_screen, left, top, SRCCOPY)
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

    # Bring to front
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

    # Bring to front
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
