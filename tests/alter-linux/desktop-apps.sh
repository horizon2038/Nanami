#!/bin/sh
# Run upstream GUI programs, requiring actual X windows rather than just PIDs.
set -eu
export DISPLAY=unix/:0
export PATH=/usr/local/bin:/bin:/usr/bin
root=/tmp/desktop-apps
mkdir -p "$root"
rm -f /tmp/desktop-apps-ok /tmp/desktop-apps-step /tmp/desktop-apps-failed
pid=
cleanup() {
    if [ -n "$pid" ]; then
        kill "$pid" 2>/dev/null || true
        wait "$pid" 2>/dev/null || true
    fi
}
finish() {
    result=$?
    trap - EXIT
    cleanup
    if [ "$result" -ne 0 ]; then
        printf '%s\n' "$application" > /tmp/desktop-apps-failed
    fi
    exit "$result"
}
trap finish EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

find_viewable_window() {
    # xwininfo also prints WM_CLASS for unnamed toolkit helper windows.
    # Match only the quoted title immediately following the window ID.
    for window in $(awk -v title="$2" '
        /^[ \t]*0x[[:xdigit:]]+[ \t]+"/ {
            name = $0
            sub(/^[ \t]*0x[[:xdigit:]]+[ \t]+"/, "", name)
            sub(/".*$/, "", name)
            if (tolower(name) ~ title) print $1
        }' "$1"); do
        # A matching window can be unmapped or disappear during startup.
        # Continue to the other titled windows before retrying the tree.
        if xwininfo -id "$window" > "$3" 2>/dev/null; then
            if grep -q 'Map State: IsViewable' "$3"; then
                return 0
            fi
        fi
    done
    return 1
}

for application in files images paint calculator clock; do
    case "$application" in
        files) title='xfe|file explorer' ;;
        images) title='logo.png' ;;
        paint) title='mtpaint' ;;
        calculator) title='galculator' ;;
        clock) title='flit' ;;
    esac
    printf '%s\n' "$application" > /tmp/desktop-apps-current
    /bin/busybox sh /usr/local/bin/nanami-app "$application" > "$root/$application.log" 2>&1 &
    pid=$!
    ready=false
    for attempt in $(seq 1 60); do
        if ! kill -0 "$pid" 2>/dev/null; then
            break
        fi
        xwininfo -root -tree > "$root/$application.tree"
        # X servers can reuse resource IDs immediately after a client exits.
        # The distinct application title, not comparison with stale IDs, is
        # what identifies this application's newly launched main window.
        if find_viewable_window "$root/$application.tree" "$title" "$root/$application.window"; then
            ready=true
            break
        fi
        sleep 1
    done
    if [ "$ready" != true ]; then
        printf 'GUI application failed: %s\n' "$application" >&2
        cat "$root/$application.log" "$root/$application.tree" >&2
        exit 1
    fi
    sleep 2
    printf '%s\n' "$application" > /tmp/desktop-apps-step
    sleep 5
    cleanup
    pid=
    sleep 0.2
done
printf 'desktop-apps-ok\n' > /tmp/desktop-apps-ok
printf 'DESKTOP-APPLICATIONS-PASS\n'
