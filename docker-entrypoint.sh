#!/bin/sh
# Fail helpfully instead of panicking.
#
# SocialSync draws a GTK window. With no reachable X server, GTK aborts
# with "Failed to initialize gtk backend!", which says nothing about the actual
# cause — a container with no display. This checks first and explains.

set -e

# Test for an actual X socket, not just the directory.
#
# docker-compose mounts /tmp/.X11-unix, and Docker CREATES that path on the
# host when it is missing — so the directory always exists inside the
# container and proves nothing. Only a socket file inside it means there is a
# real X server listening. Checking the directory let the binary start and
# panic, which is the exact failure this script exists to prevent.
has_x_socket() {
    [ -d /tmp/.X11-unix ] || return 1
    for sock in /tmp/.X11-unix/X*; do
        [ -S "$sock" ] && return 0
    done
    return 1
}

# A DISPLAY with a host part ("host.docker.internal:0", "192.168.1.5:0") means
# X over TCP, which is how VcXsrv/X410 on Windows and XQuartz on macOS work.
# There is no Unix socket in that case, so the check above would wrongly block
# a setup that actually works. Trust an explicit TCP display and let the app
# try; if the server is not really there, GTK's own error is the honest answer.
is_tcp_display() {
    case "$DISPLAY" in
        :*) return 1 ;;   # ":0" — local socket
        *:*) return 0 ;;  # "host:0" — TCP
        *) return 1 ;;
    esac
}

if [ -z "$DISPLAY" ] || { ! is_tcp_display && ! has_x_socket; }; then
    cat >&2 <<'MSG'
------------------------------------------------------------------------
SocialSync cannot start: no X display is reachable.

This is a desktop app with a native window. A container has no display of
its own, so one has to be shared in from the host.

  Linux host:
      xhost +local:docker
      docker compose up app

  macOS (XQuartz) / Windows (VcXsrv, X410):
      Start the X server, allow connections from network clients, then set a
      TCP display before starting:

          DISPLAY=host.docker.internal:0 docker compose up app

      Expect it to be slow and glitchy. Running natively is better:

          npm run tauri dev            # develop
          npm run tauri build          # produce a .app / .dmg / .msi

To build Linux binaries here instead of running the app, use:

      docker compose run --rm build     # output lands in ./dist-docker
------------------------------------------------------------------------
MSG
    exit 78 # EX_CONFIG: the environment is wrong, not the program.
fi

exec /usr/local/bin/media-downloader "$@"
