#!/bin/sh
[ ! -r /etc/profile ] || . /etc/profile
[ ! -r "$HOME/.profile" ] || . "$HOME/.profile"
unset SESSION_MANAGER DBUS_SESSION_BUS_ADDRESS WAYLAND_DISPLAY
export XDG_SESSION_TYPE=x11
export XDG_CURRENT_DESKTOP=XFCE
export XDG_SESSION_DESKTOP=xfce
export DESKTOP_SESSION=xfce
export LIBGL_ALWAYS_SOFTWARE=1
exec dbus-run-session -- startxfce4
