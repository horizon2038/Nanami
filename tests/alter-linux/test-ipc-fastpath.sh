#!/bin/sh
set -eu
repo=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
cd "$repo"
compiler=${CXX:-clang++}
test_include_root=${TEST_INCLUDE_ROOT:-spencer/A9N/src/kernel/include}
test_dir=$(mktemp -d)
case $(uname -s) in
    Darwin) strip_flag=-Wl,-dead_strip ;;
    *) strip_flag=-Wl,--gc-sections ;;
esac
sanitize_flags=
if [ "${SANITIZE:-0}" = 1 ]; then
    sanitize_flags='-fsanitize=address,undefined -fno-sanitize-recover=all'
fi
for mode in up smp; do
    smp_flag=
    if [ "$mode" = smp ]; then smp_flag=-DA9N_CONFIG_ENABLE_SMP; fi
    "$compiler" -std=c++20 -O2 -DNDEBUG -fno-exceptions -fno-rtti \
        -ffunction-sections -fdata-sections $smp_flag $sanitize_flags \
        -I "$test_include_root" \
        -I spencer/A9N/src/hal/include -I spencer/A9N/src/kernel/include \
        -I spencer/A9N/src/hal/x86_64/include -I spencer/A9N/src/liba9n/include \
        tests/alter-linux/ipc-fastpath-tests.cpp \
        spencer/A9N/src/kernel/process/scheduler.cpp \
        spencer/A9N/src/kernel/process/process_manager.cpp \
        spencer/A9N/src/kernel/capability/ipc_port.cpp \
        spencer/A9N/src/kernel/capability/notification_port.cpp \
        "$strip_flag" -o "$test_dir/ipc-$mode"
    "$test_dir/ipc-$mode"
done
printf 'Host test binaries: %s\n' "$test_dir"
