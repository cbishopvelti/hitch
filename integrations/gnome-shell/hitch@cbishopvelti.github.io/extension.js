// Hitch GNOME Shell extension.
//
// On Wayland an app cannot see other apps' windows, and cannot keep its own
// window above others. This extension runs inside GNOME Shell and does both
// for Hitch over the session bus:
//
//   * GetFocus / FocusChanged report the focused window (app id, WM class,
//     title, pid), including title changes of the focused window.
//   * SetKeepAbove pins every window of the calling process above other
//     windows and on all workspaces.
//
// Nothing is logged or stored. See README.md for the full interface.

import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import Shell from 'gi://Shell';

import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';

export const BUS_NAME = 'io.github.cbishopvelti.Hitch.Shell';
export const OBJECT_PATH = '/io/github/cbishopvelti/Hitch/Shell';
export const INTERFACE_VERSION = 1;

const INTERFACE_XML = `
<node>
  <interface name="io.github.cbishopvelti.Hitch.Shell">
    <method name="GetFocus">
      <arg type="a{sv}" name="focus" direction="out"/>
    </method>
    <method name="SetKeepAbove">
      <arg type="b" name="pinned" direction="in"/>
    </method>
    <signal name="FocusChanged">
      <arg type="a{sv}" name="focus"/>
    </signal>
    <property name="Version" type="u" access="read"/>
  </interface>
</node>`;

const EMPTY_FOCUS = Object.freeze({
    app_id: '',
    wm_class: '',
    title: '',
    pid: 0,
    sandboxed_app_id: '',
});

/**
 * Plain description of a window, safe to send over D-Bus.
 *
 * @param {Meta.Window | null} win the window
 * @returns {object} the focus fields
 */
function describeWindow(win) {
    if (!win)
        return {...EMPTY_FOCUS};

    // Shell.WindowTracker maps a window to its .desktop file, the most
    // stable app identifier. Windows it cannot match get a "window:N" id,
    // which means nothing to Hitch, so report "" for those.
    let appId = '';
    const app = Shell.WindowTracker.get_default().get_window_app(win);
    const id = app?.get_id() ?? '';
    if (id && !id.startsWith('window:'))
        appId = id.endsWith('.desktop') ? id.slice(0, -'.desktop'.length) : id;

    const pid = win.get_pid();
    return {
        app_id: appId,
        wm_class: win.get_wm_class() ?? '',
        title: win.get_title() ?? '',
        pid: pid > 0 ? pid : 0,
        sandboxed_app_id: win.get_sandboxed_app_id() ?? '',
    };
}

/**
 * @param {object} focus the focus fields
 * @returns {Object<string, GLib.Variant>} an a{sv} dictionary
 */
function focusToVariantDict(focus) {
    return {
        app_id: new GLib.Variant('s', focus.app_id),
        wm_class: new GLib.Variant('s', focus.wm_class),
        title: new GLib.Variant('s', focus.title),
        pid: new GLib.Variant('u', focus.pid),
        sandboxed_app_id: new GLib.Variant('s', focus.sandboxed_app_id),
    };
}

function sameFocus(a, b) {
    return a.app_id === b.app_id &&
        a.wm_class === b.wm_class &&
        a.title === b.title &&
        a.pid === b.pid &&
        a.sandboxed_app_id === b.sandboxed_app_id;
}

/** The D-Bus object. Method names match the interface. */
class HitchShellService {
    constructor() {
        this._lastFocus = {...EMPTY_FOCUS};
        this._focusWindow = null;
        this._focusWindowSignals = [];
        this._displaySignals = [];

        // D-Bus unique name of each caller that asked to be pinned -> {pid, watchId}.
        this._pinned = new Map();
        // Windows we made above/sticky (-> their 'unmanaged' handler id), so
        // we only undo what we did.
        this._ourWindows = new Map();

        this._dbus = Gio.DBusExportedObject.wrapJSObject(INTERFACE_XML, this);
    }

    start() {
        this._dbus.export(Gio.DBus.session, OBJECT_PATH);
        this._nameId = Gio.bus_own_name_on_connection(
            Gio.DBus.session, BUS_NAME, Gio.BusNameOwnerFlags.NONE, null, null);

        const display = global.display;
        this._displaySignals = [
            display.connect('notify::focus-window', () => this._onFocusWindowChanged()),
            display.connect('window-created', (_d, win) => this._onWindowCreated(win)),
        ];
        this._onFocusWindowChanged();
    }

    stop() {
        for (const id of this._displaySignals)
            global.display.disconnect(id);
        this._displaySignals = [];
        this._trackFocusWindow(null);

        for (const sender of [...this._pinned.keys()])
            this._unpin(sender);
        this._pinned.clear();

        if (this._nameId) {
            Gio.bus_unown_name(this._nameId);
            this._nameId = 0;
        }
        this._dbus.unexport();
    }

    // --- D-Bus interface ---------------------------------------------------

    get Version() {
        return INTERFACE_VERSION;
    }

    GetFocus() {
        return focusToVariantDict(describeWindow(global.display.focus_window));
    }

    SetKeepAboveAsync([pinned], invocation) {
        const sender = invocation.get_sender();
        if (!pinned) {
            this._unpin(sender);
            invocation.return_value(null);
            return;
        }

        Gio.DBus.session.call(
            'org.freedesktop.DBus', '/org/freedesktop/DBus', 'org.freedesktop.DBus',
            'GetConnectionUnixProcessID', new GLib.Variant('(s)', [sender]),
            new GLib.VariantType('(u)'), Gio.DBusCallFlags.NONE, -1, null,
            (conn, res) => {
                let pid;
                try {
                    [pid] = conn.call_finish(res).deepUnpack();
                } catch (e) {
                    invocation.return_error_literal(Gio.DBusError, Gio.DBusError.FAILED,
                        `Could not find the caller's process: ${e.message}`);
                    return;
                }
                this._pin(sender, pid);
                invocation.return_value(null);
            });
    }

    // --- Focus tracking ----------------------------------------------------

    _onFocusWindowChanged() {
        this._trackFocusWindow(global.display.focus_window);
        this._emitFocusIfChanged();
    }

    _trackFocusWindow(win) {
        if (this._focusWindow === win)
            return;
        for (const id of this._focusWindowSignals)
            this._focusWindow.disconnect(id);
        this._focusWindowSignals = [];
        this._focusWindow = win;
        if (!win)
            return;

        // Editors and browsers change the title when the open file or page
        // changes, and Wayland clients may set their app id after mapping.
        const update = () => this._emitFocusIfChanged();
        this._focusWindowSignals = [
            win.connect('notify::title', update),
            win.connect('notify::wm-class', update),
            win.connect('unmanaged', () => this._onFocusWindowChanged()),
        ];
    }

    _emitFocusIfChanged() {
        const focus = describeWindow(this._focusWindow);
        if (sameFocus(focus, this._lastFocus))
            return;
        this._lastFocus = focus;
        this._dbus.emit_signal('FocusChanged',
            new GLib.Variant('(a{sv})', [focusToVariantDict(focus)]));
    }

    // --- Keep above ----------------------------------------------------------

    _pin(sender, pid) {
        if (!this._pinned.has(sender)) {
            // Unpin when the caller leaves the bus, e.g. Hitch quits or crashes.
            const watchId = Gio.bus_watch_name_on_connection(
                Gio.DBus.session, sender, Gio.BusNameWatcherFlags.NONE,
                null, () => this._unpin(sender));
            this._pinned.set(sender, {pid, watchId});
        }
        for (const win of this._windowsOfPid(pid))
            this._makeAbove(win);
    }

    _unpin(sender) {
        const entry = this._pinned.get(sender);
        if (!entry)
            return;
        this._pinned.delete(sender);
        Gio.bus_unwatch_name(entry.watchId);

        const stillPinned = [...this._pinned.values()].some(e => e.pid === entry.pid);
        if (stillPinned)
            return;
        for (const win of this._windowsOfPid(entry.pid))
            this._undoAbove(win);
    }

    _isPinnedPid(pid) {
        return [...this._pinned.values()].some(e => e.pid === pid);
    }

    _windowsOfPid(pid) {
        return global.get_window_actors()
            .map(actor => actor.get_meta_window())
            .filter(win => win && win.get_pid() === pid);
    }

    _onWindowCreated(win) {
        if (this._pinned.size === 0 || !this._isPinnedPid(win.get_pid()))
            return;
        // Mutter applies the client's own initial state while placing a new
        // window, so set ours once it is shown.
        const id = win.connect('shown', () => {
            win.disconnect(id);
            if (this._isPinnedPid(win.get_pid()))
                this._makeAbove(win);
        });
    }

    _makeAbove(win) {
        if (!win.is_above())
            win.make_above();
        if (!win.is_on_all_workspaces())
            win.stick();
        if (!this._ourWindows.has(win)) {
            this._ourWindows.set(win, win.connect('unmanaged', () => {
                win.disconnect(this._ourWindows.get(win));
                this._ourWindows.delete(win);
            }));
        }
    }

    _undoAbove(win) {
        if (!this._ourWindows.has(win))
            return;
        win.disconnect(this._ourWindows.get(win));
        this._ourWindows.delete(win);
        if (win.is_above())
            win.unmake_above();
        if (win.is_on_all_workspaces())
            win.unstick();
    }
}

export default class HitchExtension extends Extension {
    enable() {
        this._service = new HitchShellService();
        this._service.start();
    }

    disable() {
        this._service?.stop();
        this._service = null;
    }
}
