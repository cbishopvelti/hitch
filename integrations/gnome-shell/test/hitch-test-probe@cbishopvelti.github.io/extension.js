// Test only: reports the shell's own view of a process's windows, so
// test/check.py can confirm the Hitch extension really kept them above, and
// switches windows and opens or closes the overview as a user would.
// (GNOME Shell 51 has no --unsafe-mode, so org.gnome.Shell.Eval is not an option.)

import Gio from 'gi://Gio';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';

import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';

const XML = `
<node>
  <interface name="io.github.cbishopvelti.HitchTestProbe">
    <method name="Windows">
      <arg type="u" name="pid" direction="in"/>
      <arg type="s" name="json" direction="out"/>
    </method>
    <method name="Activate">
      <arg type="s" name="title" direction="in"/>
    </method>
    <method name="SetOverview">
      <arg type="b" name="visible" direction="in"/>
    </method>
  </interface>
</node>`;

export default class Probe extends Extension {
    enable() {
        this._dbus = Gio.DBusExportedObject.wrapJSObject(XML, this);
        this._dbus.export(Gio.DBus.session, '/io/github/cbishopvelti/HitchTestProbe');
        this._name = Gio.bus_own_name_on_connection(Gio.DBus.session,
            'io.github.cbishopvelti.HitchTestProbe', Gio.BusNameOwnerFlags.NONE, null, null);
    }

    disable() {
        Gio.bus_unown_name(this._name);
        this._dbus.unexport();
    }

    // What a click on the window or Alt+Tab does.
    Activate(title) {
        global.get_window_actors()
            .map(a => a.get_meta_window())
            .find(w => w?.get_title() === title)
            ?.activate(global.get_current_time());
    }

    SetOverview(visible) {
        if (visible)
            Main.overview.show();
        else
            Main.overview.hide();
    }

    Windows(pid) {
        return JSON.stringify(global.get_window_actors()
            .map(a => a.get_meta_window())
            .filter(w => w && w.get_pid() === pid)
            .map(w => [w.get_title(), w.is_above(), w.is_on_all_workspaces()]));
    }
}
