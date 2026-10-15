#pragma once
#include <hal/hal_result.hpp>

namespace a9n::hal::x86_64
{
    inline bool test_pat_supported = true;
    enum class cpuid_leaf
    {
        PROCESSOR_INFO_AND_FEATURE_BITS = 1
    };

    struct cpuid_info
    {
        uint32_t rax, rbx, rcx, rdx;
    };

    inline liba9n::result<cpuid_info, hal_error> try_get_cpuid(cpuid_leaf, uint32_t)
    {
        return cpuid_info { 0, 0, 0, test_pat_supported ? 1U << 16 : 0 };
    }
}
