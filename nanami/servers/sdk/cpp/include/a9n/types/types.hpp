#ifndef A9N_TYPES_TYPES_HPP
#define A9N_TYPES_TYPES_HPP

#include <stdint.h>

namespace a9n {

using Word = uintptr_t;
using Sword = intptr_t;

enum class KernelCallType : Sword {
    CapabilityCall = -1,
    Yield = -2,
    DebugCall = -3,
};

enum class NanamiStatus : Word {
    Ok = 0,
    Unsupported = 1,
    InvalidArgument = 2,
};

} // namespace a9n

#endif
