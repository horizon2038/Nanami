#include <hal/interface/memory_manager.hpp>
#include <hal/x86_64/arch/control_register.hpp>
#include <hal/x86_64/arch/cpuid.hpp>
#include <hal/x86_64/arch/msr.hpp>
#include <hal/x86_64/memory/memory_types.hpp>
#include <hal/x86_64/memory/paging.hpp>
#include <stdio.h>
#include <stdlib.h>

using namespace a9n;
using namespace a9n::kernel;
using namespace a9n::hal::x86_64;
using liba9n::std::strcmp;
static_assert(sizeof(frame) == 3 * sizeof(word));
static_assert(MAPPING_FLAG_SHIFT == 4 && MAPPING_FLAG_MASK == 0xf0);
static_assert(mapping_cache_flags(0x7) == 0);
static_assert(mapping_cache_flags(0x17) == 8);
static_assert(mapping_cache_flags(0x27) == 24);
alignas(4096) static page pml4[512], pdpt[512], pd[512], pt[512];

namespace a9n::hal::x86_64
{
    virtual_address test_virtual_address(physical_address address)
    {
        return address;
    }
}

namespace a9n::kernel::utility
{
    void logger::printk(const char *, ...)
    {
    }

    void logger::printh(const char *, ...)
    {
    }

    void logger::error(const char *)
    {
    }
}

#define CHECK(condition) \
    do \
    { \
        if (!(condition)) \
        { \
            fprintf(stderr, "FAIL line %d: %s\n", __LINE__, #condition); \
            exit(1); \
        } \
    } while (0)

static void reset_user_tables()
{
    for (unsigned i = 0; i < 512; ++i)
    {
        pml4[i].all = pdpt[i].all = pd[i].all = pt[i].all = 0;
    }
    pml4[0].configure_physical_address(reinterpret_cast<physical_address>(pdpt));
    pml4[0].present = 1;
    pdpt[0].configure_physical_address(reinterpret_cast<physical_address>(pd));
    pdpt[0].present = 1;
    pd[0].configure_physical_address(reinterpret_cast<physical_address>(pt));
    pd[0].present = 1;
}

static void expect_mapping(unsigned bits, bool device, mapping_flag flag, word rights)
{
    reset_user_tables();
    word  va       = 0x40000000;
    word  physical = 0x80000000;
    page *leaf     = &pdpt[1];
    switch (bits)
    {
        case 12 :
            va       = 0x1000;
            physical = 0x4000;
            leaf     = &pt[1];
            break;
        case 21 :
            va       = 0x200000;
            physical = 0x800000;
            leaf     = &pd[1];
            break;
        default :
            break;
    }
    word expected = 0;
    switch (flag)
    {
        case mapping_flag::NORMAL :
            break;
        case mapping_flag::BUFFERED :
            expected = 8;
            break;
        case mapping_flag::UNCACHED :
            expected = 24;
            break;
    }
    page_table root { reinterpret_cast<physical_address>(pml4), 4 };
    frame      target { physical, bits, device ? FRAME_FLAG_DEVICE : 0 };
    test_cr3 = root.address;
    CHECK(mapping_cache_flags(with_mapping_flag(rights, flag)) == expected);
    CHECK(a9n::hal::map_frame(root, target, va, with_mapping_flag(rights, flag)));
    CHECK(leaf->present && leaf->user_supervisor);
    CHECK(bool(leaf->rw) == bool(rights & 2));
    CHECK(bool(leaf->execute_disable) == !(rights & 4));
    CHECK(leaf->get_physical_address() == physical);
    CHECK(bool(leaf->page_size) == (bits != 12));
    CHECK((leaf->all & PAGE_CACHE_MASK) == expected);
    if (bits != 12)
    {
        CHECK(!(leaf->all & (1ULL << 12)));
    }
}

int main(int argc, char **argv)
{
    CHECK(argc == 2);
    const char *mode = argv[1];
    for (word rights = 0; rights < 8; ++rights)
    {
        CHECK(with_mapping_flag(rights, mapping_flag::NORMAL) == rights);
        CHECK(with_mapping_flag(rights, mapping_flag::BUFFERED) == (rights | 0x10));
        CHECK(with_mapping_flag(rights, mapping_flag::UNCACHED) == (rights | 0x20));
        CHECK(get_mapping_flag(rights) == mapping_flag::NORMAL);
        CHECK(get_mapping_flag(rights | 0x10) == mapping_flag::BUFFERED);
        CHECK(get_mapping_flag(rights | 0x20) == mapping_flag::UNCACHED);
    }
    if (!strcmp(mode, "no-pat"))
    {
        test_pat_supported = false;
        CHECK(!configure_cpu_memory_types());
        CHECK(test_pat_reads == 0 && test_pat_writes == 0 && test_writebacks == 0);
        puts("PASS: unsupported CPU rejected before privileged state mutation");
        return 0;
    }
    if (!strcmp(mode, "pcid"))
    {
        test_cr4 |= cr4_flag::PCID;
        CHECK(!configure_cpu_memory_types());
        CHECK(test_pat_writes == 0 && test_writebacks == 0);
        puts("PASS: unsupported live PCID rejected without cache/PAT changes");
        return 0;
    }
    if (!strcmp(mode, "configured-pat"))
    {
        test_pat = 0x0007040600070106ULL;
    }
    if (!strcmp(mode, "alternate-pat"))
    {
        test_pat = 0x0606060606040006ULL;
    }
    if (!strcmp(mode, "bad-wb"))
    {
        test_pat &= ~0xffULL;
    }
    if (!strcmp(mode, "uc-minus"))
    {
        test_pat = 0x0707070707070706ULL;
    }
    const auto         original_pat     = test_pat;
    const auto         original_cr0     = test_cr0;
    const auto         original_cr4     = test_cr4;
    constexpr uint64_t selected_entries = 0xffULL | (0xffULL << 8) | (0xffULL << 24);
    if (strcmp(mode, "stateless"))
    {
        CHECK(configure_cpu_memory_types());
        CHECK((test_pat & selected_entries) == (6ULL | (1ULL << 8)));
        CHECK((test_pat & ~selected_entries) == (original_pat & ~selected_entries));
        CHECK(test_cr0 == original_cr0 && test_cr4 == original_cr4);
        CHECK(test_pat_write_safe && test_cache_disabled_during_flush);
        if (!strcmp(mode, "configured-pat"))
        {
            CHECK(test_pat_writes == 0 && test_writebacks == 0 && test_tlb_flushes == 0);
        }
        else
        {
            CHECK(test_pat_writes == 1 && test_writebacks == 2 && test_tlb_flushes == 1);
            CHECK(test_global_flushes == 1);
        }
        const auto writes = test_pat_writes;
        CHECK(configure_cpu_memory_types() && test_pat_writes == writes);
    }
    if (!strcmp(mode, "per-cpu"))
    {
        // Simulate another CPU with independent firmware PAT entries.
        test_pat                 = 0x0104060706040406ULL;
        const auto other_cpu_pat = test_pat;
        CHECK(configure_cpu_memory_types());
        CHECK((test_pat & selected_entries) == (6ULL | (1ULL << 8)));
        CHECK((test_pat & ~selected_entries) == (other_cpu_pat & ~selected_entries));
    }
    if (!strcmp(mode, "invalid-attribute"))
    {
        reset_user_tables();
        page_table root { reinterpret_cast<physical_address>(pml4), 4 };
        const word reserved[] { 0x8, 0x30, 0x40, 0xf0, 0x100, 1ULL << 63 };
        for (word flag : reserved)
        {
            CHECK(!a9n::hal::map_frame(root, frame { 0x4000, 12, 0 }, 0x1000, 3 | flag));
            CHECK(!pt[1].present);
        }
        puts("PASS: reserved attributes rejected before leaf publication");
        return 0;
    }
    const unsigned sizes[] { 12U, 21U, 30U };
    const mapping_flag flags[] { mapping_flag::NORMAL, mapping_flag::BUFFERED, mapping_flag::UNCACHED };
    for (unsigned bits : sizes)
    {
        for (mapping_flag flag : flags)
        {
            for (word rights = 0; rights < 8; ++rights)
            {
                expect_mapping(bits, false, flag, rights);
                expect_mapping(bits, true, flag, rights);
            }
        }
    }
    CHECK(test_invalidations == 144);
    puts("PASS: 144 leaf mappings; CPU-local PAT, no boot map/GOP/ready dependency");
}
