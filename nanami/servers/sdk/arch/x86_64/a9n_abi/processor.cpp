#include <a9n/abi/processor.hpp>

namespace a9n::abi {

[[noreturn]] void idle() {
    for (;;) {
        __asm__ volatile("pause");
    }
}

} // namespace a9n::abi
