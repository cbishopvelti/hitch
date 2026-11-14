#!/usr/bin/env python3
"""A stand-in for hitch-web: opens a window and asks the Hitch extension to
dock it beside the process given as the first argument. Runs until its stdin
closes. Used by check.py."""

import sys
import threading

import gi

gi.require_version("Gtk", "4.0")
from gi.repository import Gio, GLib, Gtk  # noqa: E402

beside = int(sys.argv[1])
app = Gtk.Application(application_id="io.github.cbishopvelti.HitchTestDock")


def activate(app):
    win = Gtk.ApplicationWindow(application=app, title="hitch-test-docked")
    win.set_default_size(240, 150)
    win.present()
    Gio.bus_get_sync(Gio.BusType.SESSION, None).call_sync(
        "io.github.cbishopvelti.Hitch.Shell", "/io/github/cbishopvelti/Hitch/Shell",
        "io.github.cbishopvelti.Hitch.Shell", "DockBeside",
        GLib.Variant("(u)", (beside,)), None, Gio.DBusCallFlags.NONE, 5000, None)


def quit_on_eof():
    sys.stdin.read()
    GLib.idle_add(app.quit)


app.connect("activate", activate)
threading.Thread(target=quit_on_eof, daemon=True).start()
app.run(None)
