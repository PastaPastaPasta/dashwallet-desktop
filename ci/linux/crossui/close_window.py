#!/usr/bin/env python3
"""Asks the X11 window whose title starts with TITLE to close, as a window manager's close
button does: a WM_PROTOCOLS / WM_DELETE_WINDOW client message. Xvfb runs no
window manager, so this is how the harness closes the app's window.

Usage: close_window.py [TITLE]     (default "Dash Wallet"; DISPLAY must be set)
"""

import ctypes
import subprocess
import sys

title = sys.argv[1] if len(sys.argv) > 1 else "Dash Wallet"
tree = subprocess.run(["xwininfo", "-root", "-tree"], capture_output=True, text=True).stdout
# Prefix match: the title grows to "Dash Wallet - <wallet> - [network]" (QT-011).
ids = [line.split()[0] for line in tree.splitlines() if f'"{title}' in line]
if not ids:
    print(f"close_window: no window titled {title!r}", file=sys.stderr)
    sys.exit(1)


class XClientMessageEvent(ctypes.Structure):
    _fields_ = [
        ("type", ctypes.c_int), ("serial", ctypes.c_ulong), ("send_event", ctypes.c_int),
        ("display", ctypes.c_void_p), ("window", ctypes.c_ulong), ("message_type", ctypes.c_ulong),
        ("format", ctypes.c_int), ("data", ctypes.c_long * 5),
    ]


class XEvent(ctypes.Union):
    _fields_ = [("xclient", XClientMessageEvent), ("pad", ctypes.c_long * 24)]


x11 = ctypes.CDLL("libX11.so.6")
x11.XOpenDisplay.restype = ctypes.c_void_p
x11.XOpenDisplay.argtypes = [ctypes.c_char_p]
x11.XInternAtom.restype = ctypes.c_ulong
x11.XInternAtom.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_int]
x11.XSendEvent.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_int, ctypes.c_long, ctypes.POINTER(XEvent)]
x11.XFlush.argtypes = [ctypes.c_void_p]
x11.XCloseDisplay.argtypes = [ctypes.c_void_p]

display = x11.XOpenDisplay(None)
if not display:
    print("close_window: cannot open the X display", file=sys.stderr)
    sys.exit(1)
protocols = x11.XInternAtom(display, b"WM_PROTOCOLS", 0)
delete = x11.XInternAtom(display, b"WM_DELETE_WINDOW", 0)
window = int(ids[0], 16)
event = XEvent()
event.xclient.type = 33  # ClientMessage
event.xclient.window = window
event.xclient.message_type = protocols
event.xclient.format = 32
event.xclient.data[0] = delete
event.xclient.data[1] = 0  # CurrentTime
x11.XSendEvent(display, window, 0, 0, ctypes.byref(event))
x11.XFlush(display)
x11.XCloseDisplay(display)
print(f"close_window: sent WM_DELETE_WINDOW to {ids[0]} ({title!r})")
