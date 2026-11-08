"""Cancelling a local render. Fusion runs each local render in its own RenderProcess.exe, a child of
Fusion that exits once the image is written, and runs one at a time (later renders wait in a queue).
Its API cannot cancel one: RenderFuture has no cancel method and no text command stops a render. Ending
that child process is the cancel: Fusion then drops the render job (reading its state raises "render job
doesn't exist") and writes no image.

Only a render that is processing has a process. One still queued behind another render has none yet,
so it cannot be cancelled here; the caller's staging keeps its late image away from the destination.
Windows only. Self-contained (no package imports) so it can be loaded into a modeling script as is."""
import os
import time

IMAGE = "renderprocess.exe"
# FILETIME counts 100 ns ticks from 1601-01-01; Unix time counts seconds from 1970-01-01
EPOCH_DELTA = 11644473600
# a render's process starts after startLocalRender returns; allow for clock granularity only
SLACK_SECONDS = 1.0
QUERY, TERMINATE, SYNCHRONIZE = 0x1000, 0x0001, 0x00100000


def _kernel32():
    import ctypes
    from ctypes import wintypes

    class Entry(ctypes.Structure):
        _fields_ = [("dwSize", wintypes.DWORD), ("cntUsage", wintypes.DWORD), ("th32ProcessID", wintypes.DWORD),
                    ("th32DefaultHeapID", ctypes.c_size_t), ("th32ModuleID", wintypes.DWORD), ("cntThreads", wintypes.DWORD),
                    ("th32ParentProcessID", wintypes.DWORD), ("pcPriClassBase", ctypes.c_long), ("dwFlags", wintypes.DWORD),
                    ("szExeFile", ctypes.c_wchar * 260)]

    k = ctypes.WinDLL("kernel32", use_last_error=True)
    k.CreateToolhelp32Snapshot.restype = wintypes.HANDLE
    k.CreateToolhelp32Snapshot.argtypes = (wintypes.DWORD, wintypes.DWORD)
    k.Process32FirstW.argtypes = k.Process32NextW.argtypes = (wintypes.HANDLE, ctypes.POINTER(Entry))
    k.OpenProcess.restype = wintypes.HANDLE
    k.OpenProcess.argtypes = (wintypes.DWORD, wintypes.BOOL, wintypes.DWORD)
    k.GetProcessTimes.argtypes = (wintypes.HANDLE,) + (ctypes.POINTER(wintypes.FILETIME),) * 4
    k.QueryFullProcessImageNameW.argtypes = (wintypes.HANDLE, wintypes.DWORD, wintypes.LPWSTR, ctypes.POINTER(wintypes.DWORD))
    k.TerminateProcess.argtypes = (wintypes.HANDLE, wintypes.UINT)
    k.WaitForSingleObject.restype = wintypes.DWORD
    k.WaitForSingleObject.argtypes = (wintypes.HANDLE, wintypes.DWORD)
    k.CloseHandle.argtypes = (wintypes.HANDLE,)
    return ctypes, wintypes, k, Entry


def _describe(ctypes, wintypes, k, handle, pid):
    """pid, creation time (Unix seconds) and image path of an open process handle; None if unreadable."""
    times = [wintypes.FILETIME() for _ in range(4)]
    if not k.GetProcessTimes(handle, *(ctypes.byref(t) for t in times)):
        return None
    ticks = (times[0].dwHighDateTime << 32) | times[0].dwLowDateTime
    size = wintypes.DWORD(1024)
    buffer = ctypes.create_unicode_buffer(1024)
    path = buffer.value if k.QueryFullProcessImageNameW(handle, 0, buffer, ctypes.byref(size)) else ""
    return {"pid": pid, "created": ticks / 1e7 - EPOCH_DELTA, "path": path}


def children(parent=None):
    """The RenderProcess.exe processes this Fusion started: [{pid, created, path}]."""
    if os.name != "nt":
        return []
    ctypes, wintypes, k, Entry = _kernel32()
    parent = os.getpid() if parent is None else parent
    snapshot = k.CreateToolhelp32Snapshot(0x2, 0)
    if not snapshot or snapshot == ctypes.c_void_p(-1).value:
        return []
    pids = []
    try:
        entry = Entry()
        entry.dwSize = ctypes.sizeof(Entry)
        more = k.Process32FirstW(snapshot, ctypes.byref(entry))
        while more:
            if entry.th32ParentProcessID == parent and entry.szExeFile.lower() == IMAGE:
                pids.append(entry.th32ProcessID)
            more = k.Process32NextW(snapshot, ctypes.byref(entry))
    finally:
        k.CloseHandle(snapshot)
    out = []
    for pid in pids:
        handle = k.OpenProcess(QUERY, False, pid)
        if handle:
            try:
                row = _describe(ctypes, wintypes, k, handle, pid)
            finally:
                k.CloseHandle(handle)
            if row is not None:
                out.append(row)
    return out


def terminate(pid, started, wait_ms=5000):
    """End one render process, re-checked through the handle that ends it (still a RenderProcess.exe,
    created after this render started), so a reused pid never ends anything else. (ok, how)."""
    ctypes, wintypes, k, _ = _kernel32()
    handle = k.OpenProcess(QUERY | TERMINATE | SYNCHRONIZE, False, pid)
    if not handle:
        return False, "process_gone"
    try:
        row = _describe(ctypes, wintypes, k, handle, pid)
        if row is None or not row["path"].lower().endswith("\\" + IMAGE) or row["created"] < started - SLACK_SECONDS:
            return False, "not_this_render"
        if not k.TerminateProcess(handle, 1):
            return False, "terminate_refused"
        return k.WaitForSingleObject(handle, wait_ms) == 0, "terminated"
    finally:
        k.CloseHandle(handle)


def state_of(future):
    """The render's state, or None once Fusion has dropped the job: it does when the job's process ends,
    and reading the state then raises (3 : ... render job doesn't exist)."""
    try:
        return future.renderState
    except Exception:
        return None


def cancel(future, started, states, wait=10.0, pump=None, sleep=time.sleep, clock=time.monotonic,
           find=children, end=terminate):
    """Cancel the render behind `future`, started at wall-clock `started`. states: the LocalRenderStates
    enum. Returns {cancelled, renderState, reason[, pid]}: cancelled means its process ended and Fusion
    dropped the job or reports it failed, so it writes nothing."""
    names = {states.QueuedLocalRenderState: "queued", states.ProcessingLocalRenderState: "processing",
             states.FinishedLocalRenderState: "finished", states.FailedLocalRenderState: "failed", None: "removed"}
    done = (states.FinishedLocalRenderState, states.FailedLocalRenderState, None)
    state = state_of(future)
    report = {"cancelled": False, "renderState": names.get(state, str(state))}
    if state in done:
        report["reason"] = "already_" + report["renderState"]
        return report
    if state != states.ProcessingLocalRenderState:
        # a queued render has no process yet; only the one processing (someone else's) does
        report["reason"] = "queued_behind_another_render"
        return report
    if os.name != "nt":
        report["reason"] = "unsupported_platform"
        return report
    # one render runs at a time, so while this one is processing the only render process is its own
    mine = [row for row in find() if row["created"] >= started - SLACK_SECONDS]
    if len(mine) != 1:
        report.update(reason="process_not_identified", candidates=len(mine))
        return report
    ok, how = end(mine[0]["pid"], started)
    report["pid"] = mine[0]["pid"]
    if not ok:
        report["reason"] = how
        return report
    deadline = clock() + wait
    state = state_of(future)
    while state not in done and clock() < deadline:
        if pump is not None:
            pump()
        sleep(0.2)
        state = state_of(future)
    report["renderState"] = names.get(state, str(state))
    report["cancelled"] = state in (None, states.FailedLocalRenderState)
    report["reason"] = {"removed": "process_ended", "failed": "process_ended", "finished": "finished_first"}.get(
        report["renderState"], "state_not_updated")
    return report
