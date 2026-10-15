#!/bin/sh
set -eu
repo_dir=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
a9n_dir="$repo_dir/spencer/A9N"
test_dir=$(mktemp -d /private/tmp/nanami-kernel-wakeup.XXXXXX)
compiler=${CXX:-clang++}
notification_source=${NANAMI_TEST_NOTIFICATION_SOURCE:-"$a9n_dir/src/kernel/capability/notification_port.cpp"}
test -f "$notification_source"
printf 'Notification source: %s\n' "$notification_source"
failures=0
case $(uname -s) in
    Darwin) strip_flag=-Wl,-dead_strip ;;
    *) strip_flag=-Wl,--gc-sections ;;
esac
for smp in 0 1; do
    smp_flag=
    if [ "$smp" = 1 ]; then smp_flag=-DA9N_CONFIG_ENABLE_SMP; fi
    "$compiler" -std=c++20 -O2 -DNDEBUG -fno-exceptions -fno-rtti \
        -ffunction-sections -fdata-sections $smp_flag \
        -I "$a9n_dir/src/hal/x86_64/include" \
        -I "$a9n_dir/src/hal/include" -I "$a9n_dir/src/kernel/include" \
        -I "$a9n_dir/src/liba9n/include" \
        "$repo_dir/tests/kernel-wakeup/main.cpp" \
        "$a9n_dir/src/kernel/process/process_manager.cpp" \
        "$a9n_dir/src/kernel/process/scheduler.cpp" \
        "$notification_source" \
        "$a9n_dir/src/kernel/capability/ipc_port.cpp" \
        "$strip_flag" -o "$test_dir/wakeup-$smp"
    "$test_dir/wakeup-$smp" wait-idle
    "$test_dir/wakeup-$smp" wait-busy
    "$test_dir/wakeup-$smp" bound-busy equal
    "$test_dir/wakeup-$smp" bound-busy lower
    "$test_dir/wakeup-$smp" reply-empty
    "$test_dir/wakeup-$smp" reply-pending
    for mode in bound-idle bound-busy; do
        if "$test_dir/wakeup-$smp" "$mode"; then
            printf 'SMP=%s: %s dispatched immediately\n' "$smp" "$mode"
        else
            failures=$((failures + 1))
            printf 'SMP=%s: REPRODUCED %s not dispatched on wakeup\n' "$smp" "$mode"
        fi
    done
done
printf 'Host binaries: %s\n' "$test_dir"
test "$failures" -eq 0
