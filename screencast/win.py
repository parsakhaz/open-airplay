"""Windows display enumeration via ctypes: DXGI outputs (the order ffmpeg's ddagrab uses) joined with GDI
for refresh rate, DPI scale and a readable name. Coordinates are physical pixels (per-monitor DPI aware)."""
from __future__ import annotations

import ctypes
from ctypes import wintypes

from .caps import Display

_user32 = ctypes.windll.user32
_shcore = ctypes.windll.shcore
_dxgi = ctypes.windll.dxgi

# Per-monitor-v2 awareness so every rect below is in physical pixels, not DPI-virtualized ones.
try:
    _user32.SetProcessDpiAwarenessContext(ctypes.c_void_p(-4))
except Exception:  # pragma: no cover - older Windows
    _shcore.SetProcessDpiAwareness(2)


class _GUID(ctypes.Structure):
    _fields_ = [("a", ctypes.c_uint32), ("b", ctypes.c_uint16), ("c", ctypes.c_uint16), ("d", ctypes.c_ubyte * 8)]


_IID_IDXGIFactory1 = _GUID(0x770AAE78, 0xF26F, 0x4DBA, (ctypes.c_ubyte * 8)(0xA8, 0x29, 0x25, 0x3C, 0x83, 0xD1, 0xB3, 0x87))


class _DXGI_OUTPUT_DESC(ctypes.Structure):
    _fields_ = [("DeviceName", wintypes.WCHAR * 32), ("DesktopCoordinates", wintypes.RECT),
                ("AttachedToDesktop", wintypes.BOOL), ("Rotation", ctypes.c_uint), ("Monitor", wintypes.HMONITOR)]


class _DEVMODEW(ctypes.Structure):
    _fields_ = [("dmDeviceName", wintypes.WCHAR * 32), ("dmSpecVersion", wintypes.WORD),
                ("dmDriverVersion", wintypes.WORD), ("dmSize", wintypes.WORD), ("dmDriverExtra", wintypes.WORD),
                ("dmFields", wintypes.DWORD), ("dmPositionX", ctypes.c_long), ("dmPositionY", ctypes.c_long),
                ("dmDisplayOrientation", wintypes.DWORD), ("dmDisplayFixedOutput", wintypes.DWORD),
                ("dmColor", ctypes.c_short), ("dmDuplex", ctypes.c_short), ("dmYResolution", ctypes.c_short),
                ("dmTTOption", ctypes.c_short), ("dmCollate", ctypes.c_short), ("dmFormName", wintypes.WCHAR * 32),
                ("dmLogPixels", wintypes.WORD), ("dmBitsPerPel", wintypes.DWORD), ("dmPelsWidth", wintypes.DWORD),
                ("dmPelsHeight", wintypes.DWORD), ("dmDisplayFlags", wintypes.DWORD),
                ("dmDisplayFrequency", wintypes.DWORD), ("_rest", ctypes.c_byte * 32)]


class _DISPLAY_DEVICEW(ctypes.Structure):
    _fields_ = [("cb", wintypes.DWORD), ("DeviceName", wintypes.WCHAR * 32), ("DeviceString", wintypes.WCHAR * 128),
                ("StateFlags", wintypes.DWORD), ("DeviceID", wintypes.WCHAR * 128), ("DeviceKey", wintypes.WCHAR * 128)]


class _MONITORINFO(ctypes.Structure):
    _fields_ = [("cbSize", wintypes.DWORD), ("rcMonitor", wintypes.RECT), ("rcWork", wintypes.RECT),
                ("dwFlags", wintypes.DWORD)]


def _vcall(obj, index, restype, *argtypes):
    """Call slot `index` of a COM object's vtable."""
    vtbl = ctypes.cast(obj, ctypes.POINTER(ctypes.POINTER(ctypes.c_void_p))).contents
    return ctypes.WINFUNCTYPE(restype, ctypes.c_void_p, *argtypes)(vtbl[index])


def _release(obj):
    _vcall(obj, 2, ctypes.c_ulong)(obj)


def _dxgi_outputs():
    """Outputs of adapter 0, in index order: ffmpeg's ddagrab opens the default adapter."""
    factory = ctypes.c_void_p()
    if _dxgi.CreateDXGIFactory1(ctypes.byref(_IID_IDXGIFactory1), ctypes.byref(factory)) != 0:
        return []
    adapter = ctypes.c_void_p()
    out = []
    try:
        if _vcall(factory, 12, ctypes.c_long, ctypes.c_uint, ctypes.POINTER(ctypes.c_void_p))(
                factory, 0, ctypes.byref(adapter)) != 0:  # IDXGIFactory1::EnumAdapters1
            return []
        i = 0
        while True:
            output = ctypes.c_void_p()
            if _vcall(adapter, 7, ctypes.c_long, ctypes.c_uint, ctypes.POINTER(ctypes.c_void_p))(
                    adapter, i, ctypes.byref(output)) != 0:  # IDXGIAdapter::EnumOutputs; DXGI_ERROR_NOT_FOUND ends
                break
            desc = _DXGI_OUTPUT_DESC()
            _vcall(output, 7, ctypes.c_long, ctypes.POINTER(_DXGI_OUTPUT_DESC))(output, ctypes.byref(desc))
            out.append(desc)
            _release(output)
            i += 1
    finally:
        if adapter:
            _release(adapter)
        _release(factory)
    return out


def _monitor_name(device):
    dd = _DISPLAY_DEVICEW(cb=ctypes.sizeof(_DISPLAY_DEVICEW))
    if _user32.EnumDisplayDevicesW(device, 0, ctypes.byref(dd), 0):
        return dd.DeviceString
    return device


def list_displays():
    displays = []
    for idx, d in enumerate(_dxgi_outputs()):
        if not d.AttachedToDesktop:
            continue
        mode = _DEVMODEW(dmSize=ctypes.sizeof(_DEVMODEW))
        _user32.EnumDisplaySettingsW(d.DeviceName, -1, ctypes.byref(mode))  # ENUM_CURRENT_SETTINGS
        dpi_x, dpi_y = ctypes.c_uint(96), ctypes.c_uint(96)
        _shcore.GetDpiForMonitor(d.Monitor, 0, ctypes.byref(dpi_x), ctypes.byref(dpi_y))  # MDT_EFFECTIVE_DPI
        info = _MONITORINFO(cbSize=ctypes.sizeof(_MONITORINFO))
        _user32.GetMonitorInfoW(d.Monitor, ctypes.byref(info))
        r = d.DesktopCoordinates
        displays.append(Display(
            index=idx, device=d.DeviceName, name=_monitor_name(d.DeviceName),
            x=r.left, y=r.top, width=r.right - r.left, height=r.bottom - r.top,
            refresh=int(mode.dmDisplayFrequency) or 60, scale=round(dpi_x.value / 96, 2),
            primary=bool(info.dwFlags & 1),
        ))
    return displays


_dwmapi = ctypes.windll.dwmapi


def list_windows():
    """Visible top-level windows with titles and their on-screen bounds in physical pixels."""
    found = []

    @ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
    def each(hwnd, _):
        if not _user32.IsWindowVisible(hwnd) or _user32.IsIconic(hwnd):
            return True
        n = _user32.GetWindowTextLengthW(hwnd)
        if not n:
            return True
        cloaked = wintypes.DWORD()
        _dwmapi.DwmGetWindowAttribute(hwnd, 14, ctypes.byref(cloaked), 4)  # DWMWA_CLOAKED: hidden UWP shells
        if cloaked.value:
            return True
        buf = ctypes.create_unicode_buffer(n + 1)
        _user32.GetWindowTextW(hwnd, buf, n + 1)
        r = wintypes.RECT()
        _dwmapi.DwmGetWindowAttribute(hwnd, 9, ctypes.byref(r), ctypes.sizeof(r))  # EXTENDED_FRAME_BOUNDS
        if r.right - r.left >= 64 and r.bottom - r.top >= 64:
            found.append({"hwnd": int(hwnd), "title": buf.value, "rect": (r.left, r.top, r.right, r.bottom)})
        return True

    _user32.EnumWindows(each, 0)
    return found


class _JOBOBJECT_EXTENDED_LIMIT_INFORMATION(ctypes.Structure):
    _fields_ = [("PerProcessUserTimeLimit", ctypes.c_int64), ("PerJobUserTimeLimit", ctypes.c_int64),
                ("LimitFlags", wintypes.DWORD), ("MinimumWorkingSetSize", ctypes.c_size_t),
                ("MaximumWorkingSetSize", ctypes.c_size_t), ("ActiveProcessLimit", wintypes.DWORD),
                ("Affinity", ctypes.c_size_t), ("PriorityClass", wintypes.DWORD), ("SchedulingClass", wintypes.DWORD),
                ("IoCounters", ctypes.c_uint64 * 6), ("ProcessMemoryLimit", ctypes.c_size_t),
                ("JobMemoryLimit", ctypes.c_size_t), ("PeakProcessMemoryUsed", ctypes.c_size_t),
                ("PeakJobMemoryUsed", ctypes.c_size_t)]


_kernel32 = ctypes.windll.kernel32
_kernel32.CreateJobObjectW.restype = wintypes.HANDLE
_kernel32.OpenProcess.restype = wintypes.HANDLE
_job = None


def kill_with_this_process(pid: int) -> bool:
    """Tie a child's lifetime to ours: when this process exits for any reason (crash, kill, Ctrl+C),
    Windows closes the job handle and terminates the child, so no capture/encoder is left running."""
    global _job
    if _job is None:
        job = _kernel32.CreateJobObjectW(None, None)
        info = _JOBOBJECT_EXTENDED_LIMIT_INFORMATION(LimitFlags=0x2000)  # JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
        if not job or not _kernel32.SetInformationJobObject(job, 9, ctypes.byref(info), ctypes.sizeof(info)):
            return False
        _job = job
    handle = _kernel32.OpenProcess(0x0101, False, pid)  # PROCESS_SET_QUOTA | PROCESS_TERMINATE
    if not handle:
        return False
    try:
        return bool(_kernel32.AssignProcessToJobObject(_job, handle))
    finally:
        _kernel32.CloseHandle(handle)
