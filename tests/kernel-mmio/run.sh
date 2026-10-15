#!/bin/sh
set -eu
repo_dir=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
kernel_dir="$repo_dir/spencer/A9N"
test_dir=$(mktemp -d "${TMPDIR:-/tmp}/nanami-kernel-mmio.XXXXXX")
compiler=${CXX:-clang++}
mkdir -p "$test_dir/include/hal/x86_64/arch" "$test_dir/include/hal/x86_64/memory" \
    "$test_dir/include/hal/arch" "$test_dir/include/kernel/memory"
# Mechanical adaptations of production headers: host address translation and
# privileged TLB mocks. Production page layout, traversal and CPU PAT stay intact.
for header in hal/arch/arch_types.hpp hal/x86_64/arch/arch_types.hpp; do
    perl -pe 's/0xFFFF800000000000/0/g' "$kernel_dir/src/hal/x86_64/include/$header" \
        > "$test_dir/include/$header"
done
perl -0777 -i -pe '
    s@// common@a9n::virtual_address test_virtual_address(a9n::physical_address);\n\n    // common@;
    s/return target_physical_address \| KERNEL_VIRTUAL_BASE;/return test_virtual_address(target_physical_address);/ == 1
        or die "host RAM translation mock mismatch";
' "$test_dir/include/hal/x86_64/arch/arch_types.hpp"
perl -pe 's/reinterpret_cast<a9n::virtual_address>/static_cast<a9n::virtual_address>/g' \
    "$kernel_dir/src/kernel/include/kernel/memory/memory.hpp" \
    > "$test_dir/include/kernel/memory/memory.hpp"
perl -0777 -pe '
    s/#include <stdint.h>/#include <stdint.h>\n#include <hal\/x86_64\/arch\/control_register.hpp>/;
    s/asm volatile\("mov %0, %%cr3".*?\);/test_load_cr3(cr3_address);/s == 1 or die "load_cr3 mock mismatch";
    s/asm volatile\("mov %%cr3, %0; mov %0, %%cr3".*?\);/test_flush_tlb();/s == 1 or die "flush_tlb mock mismatch";
    s/asm volatile\("invlpg \(%0\)".*?\);/test_invalidate_page(target_virtual_address);/s == 1 or die "invlpg mock mismatch";
    s/reinterpret_cast<a9n::physical_address>\(address << 12\)/static_cast<a9n::physical_address>(address << 12)/;
' "$kernel_dir/src/hal/x86_64/include/hal/x86_64/memory/paging.hpp" \
    > "$test_dir/include/hal/x86_64/memory/paging.hpp"
case $(uname -s) in
    Darwin) strip_flag=-Wl,-dead_strip ;;
    *) strip_flag=-Wl,--gc-sections ;;
esac
for smp in 0 1; do
    smp_flag=
    if [ "$smp" = 1 ]; then smp_flag=-DA9N_CONFIG_ENABLE_SMP; fi
    "$compiler" -std=c++20 -O1 -g -fno-exceptions -fno-rtti \
        -fsanitize=address,undefined -fno-omit-frame-pointer \
        -ffunction-sections -fdata-sections $smp_flag \
        -I "$test_dir/include" -I "$repo_dir/tests/kernel-mmio/include" \
        -I "$kernel_dir/src/hal/x86_64/include" -I "$kernel_dir/src/hal/include" \
        -I "$kernel_dir/src/kernel/include" -I "$kernel_dir/src/liba9n/include" \
        "$repo_dir/tests/kernel-mmio/main.cpp" \
        "$kernel_dir/src/hal/x86_64/memory/memory_manager.cpp" \
        "$strip_flag" -o "$test_dir/mmio-$smp"
    for mode in normal alternate-pat no-pat uc-minus bad-wb pcid configured-pat \
        stateless per-cpu invalid-attribute; do
        printf 'SMP=%s %s: ' "$smp" "$mode"
        "$test_dir/mmio-$smp" "$mode"
    done
done
printf 'Host binaries: %s\n' "$test_dir"
