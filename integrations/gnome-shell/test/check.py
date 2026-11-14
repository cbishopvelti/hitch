#!/usr/bin/env python3
"""End-to-end check of the Hitch GNOME Shell extension.

Run inside a GNOME Shell session that has the extension and the test-only
probe extension (test/hitch-test-probe@...) enabled; the probe lets this
script ask the shell whether windows really are kept above. run-headless.sh
sets all of that up.

The script opens GTK 4 windows as a stand-in for Hitch and checks that:
  * the extension owns its bus name and reports Version 1,
  * FocusChanged fires with the right app id, WM class, title and pid when a
    window gains focus or the focused window's title changes,
  * GetFocus agrees with the last FocusChanged,
  * SetKeepAbove(true) keeps the caller's windows above and on all
    workspaces, including windows opened afterwards,
  * SetKeepAbove(false), the caller leaving the bus, and disabling the
    extension (as GNOME does on screen lock) undo it,
  * DockBeside puts another process's window against this one's side at its
    height, moves it when this one moves or resizes, and stops when that
    process leaves the bus.
"""

import json
import os
import subprocess
import sys
import time

import gi

gi.require_version("Gtk", "4.0")
from gi.repository import Gio, GLib, Gtk  # noqa: E402

BUS_NAME = "io.github.cbishopvelti.Hitch.Shell"
OBJECT_PATH = "/io/github/cbishopvelti/Hitch/Shell"
IFACE = "io.github.cbishopvelti.Hitch.Shell"
APP_ID = "io.github.cbishopvelti.HitchTest"
PID = os.getpid()

bus = Gio.bus_get_sync(Gio.BusType.SESSION, None)
ctx = GLib.MainContext.default()
focus_events = []
failures = []


def log(msg):
    print(msg, flush=True)


def check(cond, what):
    log(("PASS " if cond else "FAIL ") + what)
    if not cond:
        failures.append(what)


def pump(seconds):
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        ctx.iteration(False)
        time.sleep(0.01)


def wait_for(pred, timeout=10.0):
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        ctx.iteration(False)
        if pred():
            return True
        time.sleep(0.01)
    return pred()


def call(method, args=None, reply="()", conn=bus):
    return conn.call_sync(
        BUS_NAME, OBJECT_PATH, IFACE, method, args,
        GLib.VariantType(reply), Gio.DBusCallFlags.NONE, 5000, None,
    ).unpack()


def our_windows():
    """[(title, above, sticky)] for this process's windows, asked of the shell
    through the test-only probe extension."""
    (out,) = bus.call_sync(
        "io.github.cbishopvelti.HitchTestProbe", "/io/github/cbishopvelti/HitchTestProbe",
        "io.github.cbishopvelti.HitchTestProbe", "Windows",
        GLib.Variant("(u)", (PID,)), GLib.VariantType("(s)"),
        Gio.DBusCallFlags.NONE, 5000, None,
    ).unpack()
    return json.loads(out)


def probe(method, args=None, reply="()"):
    return bus.call_sync(
        "io.github.cbishopvelti.HitchTestProbe", "/io/github/cbishopvelti/HitchTestProbe",
        "io.github.cbishopvelti.HitchTestProbe", method, args, GLib.VariantType(reply),
        Gio.DBusCallFlags.NONE, 5000, None).unpack()



def rects(pid):
    """{title: (x, y, width, height)} for a process's windows."""
    (out,) = probe("Rects", GLib.Variant("(u)", (pid,)), "(s)")
    return {t: tuple(r) for t, *r in json.loads(out)}


def docked_to(panel, docked):
    """Whether `docked` touches a side of `panel` at its top and height."""
    if not panel or not docked:
        return False
    px, py, pw, ph = panel
    dx, dy, dw, dh = docked
    return (dx + dw == px or dx == px + pw) and dy == py and dh == ph


def last_focus_with_title(title):
    for f in reversed(focus_events):
        if f.get("title") == title:
            return f
    return None


def on_signal(_conn, _sender, _path, _iface, _signal, params):
    (focus,) = params.unpack()
    focus_events.append(focus)
    log(f"     FocusChanged {focus}")


def main():
    check(wait_for(lambda: name_has_owner(), 30.0), "extension owns its bus name")
    if failures:
        return

    version = bus.call_sync(
        BUS_NAME, OBJECT_PATH, "org.freedesktop.DBus.Properties", "Get",
        GLib.Variant("(ss)", (IFACE, "Version")), GLib.VariantType("(v)"),
        Gio.DBusCallFlags.NONE, 5000, None,
    ).unpack()[0]
    check(version == 1, f"Version property is 1 (got {version})")

    bus.signal_subscribe(BUS_NAME, IFACE, "FocusChanged", OBJECT_PATH, None,
                         Gio.DBusSignalFlags.NONE, on_signal)

    app = Gtk.Application(application_id=APP_ID)
    app.register(None)
    windows = []

    def open_window(title):
        win = Gtk.ApplicationWindow(application=app, title=title)
        win.set_default_size(300, 200)
        win.present()
        windows.append(win)
        return win

    # GNOME Shell starts in the overview; leave it, as a user does.
    probe("SetOverview", GLib.Variant("(b)", (False,)))
    pump(1)

    # 1. Focus a new window.
    win_a = open_window("hitch-test-A")
    check(wait_for(lambda: last_focus_with_title("hitch-test-A")),
          "FocusChanged fires when window A gains focus")
    f = last_focus_with_title("hitch-test-A") or {}
    check(f.get("pid") == PID, f"pid is this process ({f.get('pid')} == {PID})")
    check(f.get("wm_class") == APP_ID, f"wm_class is the Wayland app id ({f.get('wm_class')!r})")
    check(f.get("app_id") == APP_ID, f"app_id comes from the .desktop file ({f.get('app_id')!r})")
    check(f.get("sandboxed_app_id") == "", "sandboxed_app_id is empty for an unsandboxed app")
    check(set(f) == {"app_id", "wm_class", "title", "pid", "sandboxed_app_id"},
          f"focus has exactly the documented keys ({sorted(f)})")

    (got,) = call("GetFocus", reply="(a{sv})")
    check(got.get("title") == "hitch-test-A" and got.get("pid") == PID,
          f"GetFocus agrees ({got})")

    # 2. Title change of the focused window (an editor opening another file).
    win_a.set_title("hitch-test-A renamed")
    check(wait_for(lambda: last_focus_with_title("hitch-test-A renamed")),
          "FocusChanged fires when the focused window's title changes")

    # 3. Focus moves to another window.
    open_window("hitch-test-B")
    check(wait_for(lambda: last_focus_with_title("hitch-test-B")),
          "FocusChanged fires when focus moves to window B")

    # Switching back to an existing window, as a click or Alt+Tab does.
    n = len(focus_events)
    probe("Activate", GLib.Variant("(s)", ("hitch-test-A renamed",)))
    check(wait_for(lambda: len(focus_events) > n
                   and focus_events[-1].get("title") == "hitch-test-A renamed"),
          "FocusChanged fires when switching back to window A")
    probe("Activate", GLib.Variant("(s)", ("hitch-test-B",)))
    check(wait_for(lambda: focus_events[-1].get("title") == "hitch-test-B"),
          "FocusChanged fires when switching to window B again")

    # No duplicates: nothing is sent while nothing changes.
    n = len(focus_events)
    pump(0.5)
    check(len(focus_events) == n, "no FocusChanged while nothing changes")

    # Opening and closing the overview keeps the window focused, so Hitch
    # hears nothing and keeps showing the same context.
    n = len(focus_events)
    probe("SetOverview", GLib.Variant("(b)", (True,)))
    pump(1)
    probe("SetOverview", GLib.Variant("(b)", (False,)))
    pump(1)
    check(len(focus_events) == n, "the overview does not change the reported focus")

    # 4. Pin.
    check(all(not above for _, above, _ in our_windows()), "windows start not above")
    call("SetKeepAbove", GLib.Variant("(b)", (True,)))
    check(wait_for(lambda: all(a and s for _, a, s in our_windows()), 5.0),
          f"SetKeepAbove(true) makes every window above and sticky ({our_windows()})")

    open_window("hitch-test-C")
    check(wait_for(lambda: any(t == "hitch-test-C" and a and s for t, a, s in our_windows()), 5.0),
          f"a window opened while pinned is above and sticky too ({our_windows()})")

    # 5. Unpin.
    call("SetKeepAbove", GLib.Variant("(b)", (False,)))
    check(wait_for(lambda: all(not a and not s for _, a, s in our_windows()), 5.0),
          f"SetKeepAbove(false) undoes it ({our_windows()})")

    # 5b. Dock another process's window beside window A (the branch page
    # beside the panel).
    panel = "hitch-test-A renamed"

    def panel_rect():
        return rects(PID).get(panel)

    def at(x, y):
        return (panel_rect() or (None, None))[:2] == (x, y)

    probe("MoveResize", GLib.Variant("(siiii)", (panel, 700, 120, 320, 400)))
    check(wait_for(lambda: at(700, 120), 5.0), f"the probe moved window A ({panel_rect()})")
    child = subprocess.Popen(
        [sys.executable, "-I",
         os.path.join(os.path.dirname(os.path.abspath(__file__)), "dock_child.py"), str(PID)],
        stdin=subprocess.PIPE)

    def docked_rect():
        return rects(child.pid).get("hitch-test-docked")

    def docked():
        return docked_to(panel_rect(), docked_rect())

    check(wait_for(docked, 10.0),
          f"DockBeside puts the caller's window against the other's side at its height "
          f"({panel_rect()}, {docked_rect()})")
    d = docked_rect() or (0, 0, 0, 0)
    check(d[0] + d[2] == 700, f"on the side with more room, the left ({d})")
    width = d[2]

    probe("MoveResize", GLib.Variant("(siiii)", (panel, 100, 60, 320, 500)))
    check(wait_for(lambda: at(100, 60) and docked(), 5.0),
          f"it follows when the other window moves and resizes ({panel_rect()}, {docked_rect()})")
    p, d = panel_rect() or (0, 0, 0, 0), docked_rect() or (0, 0, 0, 0)
    check(d[0] == p[0] + p[2], f"and changes side when the room does ({p}, {d})")
    check(d[2] == width, f"its width is kept ({d[2]} == {width})")

    child.stdin.close()
    child.wait(10)
    probe("MoveResize", GLib.Variant("(siiii)", (panel, 300, 60, 320, 400)))
    check(wait_for(lambda: at(300, 60), 5.0),
          f"the other window still moves after the docked one closes ({panel_rect()})")

    # 6. Pin from a second connection, then drop that connection: the
    # extension must unpin when the caller leaves the bus.
    addr = Gio.dbus_address_get_for_bus_sync(Gio.BusType.SESSION, None)
    conn2 = Gio.DBusConnection.new_for_address_sync(
        addr,
        Gio.DBusConnectionFlags.AUTHENTICATION_CLIENT
        | Gio.DBusConnectionFlags.MESSAGE_BUS_CONNECTION,
        None, None)
    call("SetKeepAbove", GLib.Variant("(b)", (True,)), conn=conn2)
    check(wait_for(lambda: all(a for _, a, _ in our_windows()), 5.0),
          "pinned from a second connection")
    conn2.close_sync(None)
    check(wait_for(lambda: all(not a and not s for _, a, s in our_windows()), 5.0),
          f"unpinned when the caller leaves the bus ({our_windows()})")

    # 7. GNOME disables extensions while the screen is locked. Disabling must
    # undo the pin and drop the bus name; enabling must bring the name back.
    call("SetKeepAbove", GLib.Variant("(b)", (True,)))
    wait_for(lambda: all(a for _, a, _ in our_windows()), 5.0)
    shell_extensions("DisableExtension")
    check(wait_for(lambda: not name_has_owner(), 5.0), "disabling drops the bus name")
    check(wait_for(lambda: all(not a and not s for _, a, s in our_windows()), 5.0),
          f"disabling unpins the windows ({our_windows()})")
    shell_extensions("EnableExtension")
    check(wait_for(name_has_owner, 5.0), "enabling again brings the bus name back")
    (got,) = call("GetFocus", reply="(a{sv})")
    check(got.get("pid") == PID, f"GetFocus works after re-enabling ({got})")

    # 8. Nothing focused once all windows close.
    for w in windows:
        w.destroy()
    check(wait_for(lambda: focus_events and focus_events[-1].get("title") == "", 5.0),
          f"FocusChanged reports an empty focus when no window is focused ({focus_events[-1:]})")


def shell_extensions(method):
    bus.call_sync(
        "org.gnome.Shell.Extensions", "/org/gnome/Shell/Extensions", "org.gnome.Shell.Extensions",
        method, GLib.Variant("(s)", ("hitch@cbishopvelti.github.io",)), GLib.VariantType("(b)"),
        Gio.DBusCallFlags.NONE, 5000, None)


def name_has_owner():
    return bus.call_sync(
        "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus",
        "NameHasOwner", GLib.Variant("(s)", (BUS_NAME,)), GLib.VariantType("(b)"),
        Gio.DBusCallFlags.NONE, 5000, None,
    ).unpack()[0]


if __name__ == "__main__":
    try:
        main()
    except Exception as e:  # report, don't hide, unexpected errors
        failures.append(repr(e))
        log(f"FAIL unexpected error: {e!r}")
    log(f"\n{len(failures)} failure(s)")
    sys.exit(1 if failures else 0)
