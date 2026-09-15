#!/bin/sh
# Run inside a Linux environment with gcc and glibc development files.
set -eu
test_source=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
fixture_root=${1:?usage: build-glibc-fixtures.sh output-root}
mkdir -p "$fixture_root/bin" "$fixture_root/lib"
cp /usr/bin/true "$fixture_root/bin/glibc-true"
gcc -O2 -Wall -Wextra -Werror -fPIC -shared "$test_source/probe.c" -o "$fixture_root/lib/libalter-probe.so"
gcc -O2 -Wall -Wextra -Werror -fPIE -pie "$test_source/glibc-regression.c" -ldl -o "$fixture_root/bin/glibc-regression"
gcc -O2 -Wall -Wextra -Werror -fPIE -pie -Wl,--dynamic-linker=/lib/absent-interpreter.so "$test_source/glibc-regression.c" -ldl -o "$fixture_root/bin/glibc-missing-interp"
case $(uname -m) in
  x86_64)
    mkdir -p "$fixture_root/lib64" "$fixture_root/lib/x86_64-linux-gnu"
    cp -L /lib64/ld-linux-x86-64.so.2 "$fixture_root/lib64/"
    cp -L /lib/x86_64-linux-gnu/libc.so.6 "$fixture_root/lib/x86_64-linux-gnu/"
    ;;
  aarch64)
    mkdir -p "$fixture_root/lib/aarch64-linux-gnu"
    cp -L /lib/ld-linux-aarch64.so.1 "$fixture_root/lib/"
    cp -L /lib/aarch64-linux-gnu/libc.so.6 "$fixture_root/lib/aarch64-linux-gnu/"
    ;;
  *) echo 'unsupported fixture architecture' >&2; exit 1 ;;
esac
