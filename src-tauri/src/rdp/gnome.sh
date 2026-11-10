#!/bin/sh
[ ! -r /etc/profile ] || . /etc/profile
[ ! -r "$HOME/.profile" ] || . "$HOME/.profile"

unset SESSION_MANAGER WAYLAND_DISPLAY

# Select the Ubuntu GNOME desktop and its installed NVIDIA customizations.
export XDG_SESSION_TYPE=x11
export DESKTOP_SESSION=ubuntu-xorg
export XDG_SESSION_DESKTOP=ubuntu-xorg
export XDG_CURRENT_DESKTOP=ubuntu:GNOME
export GNOME_SHELL_SESSION_MODE=ubuntu
export XDG_CONFIG_DIRS=/etc/xdg/xdg-ubuntu-xorg:/etc/xdg
export XDG_DATA_DIRS=/usr/share/ubuntu-xorg:/usr/share/gnome:/usr/local/share:/usr/share:/var/lib/snapd/desktop

# Use the display credentials and user bus that fixed Firefox.
export XAUTHORITY="$HOME/.Xauthority"
export XDG_RUNTIME_DIR="/run/user/$(id -u)"
export DBUS_SESSION_BUS_ADDRESS="unix:path=$XDG_RUNTIME_DIR/bus"
export LIBGL_ALWAYS_SOFTWARE=1

if [ ! -S "$XDG_RUNTIME_DIR/bus" ]; then
    echo "The GNOME user-session bus is unavailable: $XDG_RUNTIME_DIR/bus" >&2
    exit 1
fi

# Give applications started by D-Bus/systemd the same desktop environment.
systemctl --user unset-environment WAYLAND_DISPLAY SESSION_MANAGER
dbus-update-activation-environment --systemd \
    DBUS_SESSION_BUS_ADDRESS DISPLAY XAUTHORITY XDG_RUNTIME_DIR \
    XDG_SESSION_TYPE DESKTOP_SESSION XDG_SESSION_DESKTOP \
    XDG_CURRENT_DESKTOP GNOME_SHELL_SESSION_MODE \
    XDG_CONFIG_DIRS XDG_DATA_DIRS LIBGL_ALWAYS_SOFTWARE || exit 1

exec /usr/bin/gnome-session --session=ubuntu
