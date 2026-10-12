# Keeps a paste from the owner's computer fresh in the desktop's apps.
#
# x11vnc takes the CLIPBOARD selection for the first paste from the viewer
# and keeps it: later pastes only swap the text it serves, so the X
# selection owner never changes. Chromium caches the clipboard until the
# owner changes and keeps pasting the first text. Whenever x11vnc becomes
# the owner, xclip takes the same text over. x11vnc loses the selection and
# takes it again on the next paste, which every app sees as new.
#
# Standard library only: libX11/libXfixes through ctypes. The text is never
# printed or written anywhere. Exits when the display goes away.
import ctypes
import subprocess
import sys

c_ulong, c_int, c_void_p = ctypes.c_ulong, ctypes.c_int, ctypes.c_void_p
try:
    X = ctypes.CDLL("libX11.so.6")
    XF = ctypes.CDLL("libXfixes.so.3")
except OSError:
    sys.exit(0)

X.XOpenDisplay.restype = c_void_p
X.XOpenDisplay.argtypes = [ctypes.c_char_p]
X.XDefaultRootWindow.restype = c_ulong
X.XDefaultRootWindow.argtypes = [c_void_p]
X.XInternAtom.restype = c_ulong
X.XInternAtom.argtypes = [c_void_p, ctypes.c_char_p, c_int]
X.XNextEvent.argtypes = [c_void_p, c_void_p]
X.XGetGeometry.argtypes = [c_void_p, c_ulong] + [c_void_p] * 7
X.XSetErrorHandler.restype = c_void_p
XF.XFixesQueryExtension.argtypes = [c_void_p, c_void_p, c_void_p]
XF.XFixesSelectSelectionInput.argtypes = [c_void_p, c_ulong, c_ulong, c_ulong]

dpy = X.XOpenDisplay(None)
if not dpy:
    sys.exit(0)
# A window that is gone by the time it is measured is not fatal.
ERROR_HANDLER = ctypes.CFUNCTYPE(c_int, c_void_p, c_void_p)(lambda d, e: 0)
X.XSetErrorHandler(ERROR_HANDLER)

root = X.XDefaultRootWindow(dpy)
clipboard = X.XInternAtom(dpy, b"CLIPBOARD", 0)
event_base, error_base = c_int(), c_int()
if not XF.XFixesQueryExtension(dpy, ctypes.byref(event_base), ctypes.byref(error_base)):
    sys.exit(0)
XF.XFixesSelectSelectionInput(dpy, root, clipboard, 1)  # SetSelectionOwnerNotifyMask


class SelectionNotify(ctypes.Structure):
    _fields_ = [
        ("type", c_int),
        ("serial", c_ulong),
        ("send_event", c_int),
        ("display", c_void_p),
        ("window", c_ulong),
        ("subtype", c_int),
        ("owner", c_ulong),
        ("selection", c_ulong),
        ("timestamp", c_ulong),
        ("selection_timestamp", c_ulong),
    ]


def is_x11vnc(window):
    """x11vnc owns selections with a 1x1 window it creates at (3, 2)."""
    r, x, y, w, h, bw, depth = c_ulong(), c_int(), c_int(), ctypes.c_uint(), ctypes.c_uint(), ctypes.c_uint(), ctypes.c_uint()
    ok = X.XGetGeometry(dpy, window, *(ctypes.byref(v) for v in (r, x, y, w, h, bw, depth)))
    return bool(ok) and (x.value, y.value, w.value, h.value) == (3, 2, 1, 1)


event = ctypes.create_string_buffer(256)  # XEvent: 24 longs
while True:
    X.XNextEvent(dpy, event)
    ev = SelectionNotify.from_buffer(event)
    if ev.type != event_base.value or ev.selection != clipboard or not ev.owner:
        continue
    if is_x11vnc(ev.owner):
        subprocess.run(
            "xclip -o -selection clipboard | xclip -selection clipboard",
            shell=True,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            timeout=5,
        )
