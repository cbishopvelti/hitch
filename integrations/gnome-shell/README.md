# Hitch GNOME Shell extension

On Wayland, which is the only session on current Ubuntu, an app cannot see
which window is focused and cannot keep its own window above others. This
small GNOME Shell extension does both for Hitch:

- it reports the focused window (app, WM class, title, pid) and every change
  to it, including title changes when an editor opens another file or a
  browser switches tab;
- it keeps the Hitch panel above other windows and on every workspace while
  the panel is pinned.

Hitch talks to it over the session bus. Nothing is logged or stored.

Supported: GNOME Shell 50 (Ubuntu 26.04 LTS) and 51 (Ubuntu 26.10).

## Install (development)

```sh
make -C integrations/gnome-shell install   # copies into ~/.local/share/gnome-shell/extensions
```

Then log out and back in (Wayland cannot restart the shell in place), and:

```sh
gnome-extensions enable hitch@cbishopvelti.github.io
```

`make pack` builds `dist/hitch@cbishopvelti.github.io.shell-extension.zip`,
which `gnome-extensions install` and extensions.gnome.org accept. CI also
attaches it to every run.

Check it is running:

```sh
gdbus call --session -d io.github.cbishopvelti.Hitch.Shell \
  -o /io/github/cbishopvelti/Hitch/Shell \
  -m io.github.cbishopvelti.Hitch.Shell.GetFocus
gdbus monitor --session -d io.github.cbishopvelti.Hitch.Shell   # watch FocusChanged
```

## D-Bus interface

| | |
|---|---|
| Bus | session |
| Bus name | `io.github.cbishopvelti.Hitch.Shell` |
| Object path | `/io/github/cbishopvelti/Hitch/Shell` |
| Interface | `io.github.cbishopvelti.Hitch.Shell` |

```xml
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
```

**`focus`** always has these keys. When nothing is focused (empty desktop)
the strings are empty and `pid` is 0.

| Key | Type | Meaning |
|---|---|---|
| `app_id` | `s` | The app's `.desktop` file id without `.desktop`, e.g. `dev.zed.Zed`, `org.gnome.Ptyxis`. `""` if GNOME cannot match the window to an installed app. Best key for app rules. |
| `wm_class` | `s` | The Wayland app id (or X11 `WM_CLASS` for Xwayland windows). Fallback when `app_id` is empty. |
| `title` | `s` | The window title. |
| `pid` | `u` | The window's process id, 0 if unknown. |
| `sandboxed_app_id` | `s` | The Flatpak or Snap app id, `""` if not sandboxed. |

**`FocusChanged(focus)`** is emitted when another window gains focus and when
the focused window's title or WM class changes. It is not repeated when
nothing changed. Opening the overview does not change it: the window that
was focused stays reported until another one takes focus.

**`GetFocus() -> focus`** returns the current focus, for when Hitch starts.

**`SetKeepAbove(pinned)`**: with `true`, every window of the *calling
process* (found from the caller's D-Bus connection, so one app cannot pin
another's windows) is kept above other windows and shown on all workspaces,
including windows it opens later. `false` undoes it. The pin is also dropped
when the caller disconnects from the bus, so a crashed Hitch never leaves a
window stuck on top.

**`Version`** is 1. It goes up when the interface changes incompatibly.

### What Hitch (the Rust side) should do

1. Watch the bus name. Whenever it gains an owner (at Hitch startup, after the
   extension is enabled, and after the screen unlocks, because GNOME disables
   extensions while the screen is locked), call `GetFocus`, subscribe to
   `FocusChanged`, and if the panel is pinned call `SetKeepAbove(true)` again.
2. Ignore focus whose `pid` is Hitch's own: that is the panel itself, and the
   last context should stay shown.
3. If the name has no owner, the extension is not installed or not enabled;
   offer to install it (spec: permissions onboarding).

[`zbus`](https://crates.io/crates/zbus) handles all of this; `a{sv}` maps to
`HashMap<String, OwnedValue>`.

## Privacy

Window titles can be sensitive. The extension only sends them over the local
session bus, which other apps running as the same user can also listen to
(the same apps could already read your files). Sandboxed Flatpak and Snap
apps cannot see it unless they were granted session bus access. Nothing is
written to disk or to the journal.

## Tests

`make test` (`test/run-headless.sh`) starts a real GNOME Shell in headless
mode with its own session bus, HOME and settings, enables the extension plus
a test-only probe extension (`test/hitch-test-probe@...`, which reports the
shell's own view of the windows), and runs `test/check.py`. That script opens
GTK 4 windows as a stand-in for Hitch and checks focus reports, title
changes, the overview, pinning (including windows opened while pinned),
unpinning, unpin when the caller disconnects, and disable/enable as on
screen lock.

CI runs it on GNOME Shell 50 (`ubuntu:26.04`) and 51 (`ubuntu:26.10`)
containers, see `.github/workflows/gnome-shell.yml`, and uploads the packed
extension. To run it locally in a throwaway container:

```sh
docker run --rm -v "$PWD:/src" -w /src ubuntu:26.10 sh -c \
  'apt-get update && apt-get install -y --no-install-recommends $(cat integrations/gnome-shell/test/packages.txt) \
   && integrations/gnome-shell/test/run-headless.sh'
```

What the headless test cannot cover: a real GPU and monitor, keyboard and
mouse focus changes (focus here moves because new windows take it), a
logged-in Ubuntu session with Ubuntu's own extensions (dock, tiling
assistant), Xwayland windows, and Flatpak or Snap apps.
