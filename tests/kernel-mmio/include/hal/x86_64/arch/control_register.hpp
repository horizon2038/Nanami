#pragma once
#include <kernel/types.hpp>

namespace a9n::hal::x86_64
{
    inline uint64_t test_cr0                         = 0;
    inline uint64_t test_cr3                         = 0;
    inline uint64_t test_cr4                         = 1ULL << 7;
    inline unsigned test_writebacks                  = 0;
    inline unsigned test_tlb_flushes                 = 0;
    inline unsigned test_global_flushes              = 0;
    inline unsigned test_invalidations               = 0;
    inline bool     test_cache_disabled_during_flush = true;

    inline void _writeback_invalidate_caches()
    {
        ++test_writebacks;
        test_cache_disabled_during_flush &= (test_cr0 & (1ULL << 30)) && !(test_cr0 & (1ULL << 29));
    }

    inline uint64_t _read_cr0()
    {
        return test_cr0;
    }

    inline void _write_cr0(uint64_t value)
    {
        test_cr0 = value;
    }

    inline uint64_t _read_cr3()
    {
        return test_cr3;
    }

    inline void _write_cr3(uint64_t value)
    {
        test_cr3 = value;
        ++test_tlb_flushes;
    }

    inline uint64_t _read_cr4()
    {
        return test_cr4;
    }

    inline void _write_cr4(uint64_t value)
    {
        if ((test_cr4 & (1ULL << 7)) && !(value & (1ULL << 7)))
        {
            ++test_global_flushes;
        }
        test_cr4 = value;
    }

    inline void test_load_cr3(uint64_t value)
    {
        _write_cr3(value);
    }

    inline void test_flush_tlb()
    {
        ++test_tlb_flushes;
    }

    inline void test_invalidate_page(a9n::word)
    {
        ++test_invalidations;
    }

    namespace cr0_flag
    {
        inline constexpr uint64_t CACHE_DISABLE     = 1ULL << 30;
        inline constexpr uint64_t NOT_WRITE_THROUGH = 1ULL << 29;
    }

    namespace cr4_flag
    {
        inline constexpr uint64_t PAGE_GLOBAL = 1ULL << 7;
        inline constexpr uint64_t PCID        = 1ULL << 17;
    }
}
