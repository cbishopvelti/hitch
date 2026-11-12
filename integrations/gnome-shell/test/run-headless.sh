#!/bin/sh
# Runs the extension inside a real, headless GNOME Shell and checks it with
# test/check.py. Needs a system with GNOME Shell 50 or 51 and the packages in
# test/packages.txt (CI uses an ubuntu:26.10 container). Nothing outside a
# temporary directory is touched: it uses its own HOME, session bus and
# settings.
set -eu

HERE=$(cd "$(dirname "$0")/.." && pwd)
UUID=hitch@cbishopvelti.github.io
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

export HOME="$WORK/home"
export XDG_RUNTIME_DIR="$WORK/run"
export XDG_DATA_HOME="$HOME/.local/share"
export XDG_CONFIG_HOME="$HOME/.config"
export GSETTINGS_BACKEND=keyfile
export NO_AT_BRIDGE=1 GTK_A11Y=none
mkdir -p "$XDG_RUNTIME_DIR" "$XDG_DATA_HOME/gnome-shell/extensions" \
    "$XDG_DATA_HOME/applications" "$XDG_CONFIG_HOME/glib-2.0/settings"
chmod 700 "$XDG_RUNTIME_DIR"

PROBE=hitch-test-probe@cbishopvelti.github.io
cp -r "$HERE/$UUID" "$XDG_DATA_HOME/gnome-shell/extensions/$UUID"
cp -r "$HERE/test/$PROBE" "$XDG_DATA_HOME/gnome-shell/extensions/$PROBE"

# A .desktop file so Shell.WindowTracker can match the test window to an app.
cat > "$XDG_DATA_HOME/applications/io.github.cbishopvelti.HitchTest.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Hitch test window
Exec=true
EOF

cat > "$XDG_CONFIG_HOME/glib-2.0/settings/keyfile" <<EOF
[org/gnome/shell]
enabled-extensions=['$UUID', '$PROBE']
disable-user-extensions=false
welcome-dialog-last-shown-version='999'
EOF

# GNOME Shell needs a system bus too. Containers usually have none, so start
# one (only possible as root, which a CI container is). A container has no
# logind either, and GNOME Shell only looks for logind when
# /run/systemd/seats exists, so remove that (empty) directory.
if [ ! -S /run/dbus/system_bus_socket ] && [ "$(id -u)" = 0 ]; then
    mkdir -p /run/dbus
    dbus-daemon --system --fork
    rmdir /run/systemd/seats 2>/dev/null || true
fi

echo "GNOME Shell: $(gnome-shell --version)"

export HITCH_TEST_DIR="$HERE/test" HITCH_WORK="$WORK"
dbus-run-session -- sh -eu -c '
    gnome-shell --headless --virtual-monitor 1280x800 \
        > "$HITCH_WORK/shell.log" 2>&1 &
    shell=$!

    i=0
    until [ -S "$XDG_RUNTIME_DIR/wayland-0" ]; do
        i=$((i + 1))
        if [ $i -gt 300 ] || ! kill -0 $shell 2>/dev/null; then
            echo "GNOME Shell did not start:"; cat "$HITCH_WORK/shell.log"; exit 1
        fi
        sleep 0.1
    done

    status=0
    WAYLAND_DISPLAY=wayland-0 GDK_BACKEND=wayland GSK_RENDERER=cairo \
        python3 -I "$HITCH_TEST_DIR/check.py" || status=$?

    if [ $status -ne 0 ] || grep -qi "hitch" "$HITCH_WORK/shell.log"; then
        echo "--- GNOME Shell log (lines about the extension) ---"
        grep -i -B2 -A8 "hitch\|JS ERROR\|extension" "$HITCH_WORK/shell.log" || true
    fi
    kill $shell 2>/dev/null || true
    wait $shell 2>/dev/null || true
    exit $status
'
