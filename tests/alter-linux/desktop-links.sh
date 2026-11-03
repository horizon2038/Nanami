#!/bin/sh
# Run in the real Tiny Core X terminal, twice in one QEMU snapshot.
set -eu
cycle=$1
for entry in Terminal:aterm Files:xfe Editor:editor Images:flpicsee Paint:mtpaint Calculator:galculator; do
    test "$(readlink "/root/.wmx/${entry%%:*}")" = "/usr/local/bin/${entry#*:}"
done
case "$cycle" in
    1)
        test "$(readlink /root/.wmx/Clock)" = /usr/local/bin/flit
        test "$(readlink /root/Pictures/TinyCore.png)" = /usr/local/share/pixmaps/logo.png
        # Change both a menu alias and a picture file before Exit/restart.
        ln -sf /usr/local/bin/aterm /root/.wmx/Clock
        rm /root/Pictures/TinyCore.png
        printf '%s\n' custom-picture > /root/Pictures/TinyCore.png
        ;;
    2)
        test "$(readlink /root/.wmx/Clock)" = /usr/local/bin/aterm
        test ! -L /root/Pictures/TinyCore.png
        test "$(cat /root/Pictures/TinyCore.png)" = custom-picture
        ;;
    *) exit 1 ;;
esac
printf 'desktop-links-ok-%s\n' "$cycle" > /tmp/desktop-links-ok
