#pragma once
#include <hal/x86_64/arch/control_register.hpp>
#include <stdint.h>

namespace a9n::hal::x86_64
{
    inline uint64_t test_pat            = 0x0007040600070406ULL;
    inline unsigned test_pat_reads      = 0;
    inline unsigned test_pat_writes     = 0;
    inline bool     test_pat_write_safe = true;

    inline uint64_t _read_msr(uint32_t)
    {
        ++test_pat_reads;
        return test_pat;
    }

    inline void _write_msr(uint32_t index, uint64_t value)
    {
        ++test_pat_writes;
        test_pat_write_safe
            &= index == 0x277 && (test_cr0 & (1ULL << 30)) && !(test_cr0 & (1ULL << 29))
            && !(test_cr4 & (1ULL << 7)) && test_writebacks > 0;
        test_pat = value;
    }

    namespace msr
    {
        inline constexpr uint32_t PAT = 0x277;
    }
}
