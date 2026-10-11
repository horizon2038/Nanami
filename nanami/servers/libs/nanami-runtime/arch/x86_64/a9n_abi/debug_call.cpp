#include <a9n/abi/debug_call.hpp>
#include <a9n/types/types.hpp>

namespace a9n::abi {

void debug_write_char(char c) {
    register a9n::Sword kernel_call_no __asm__("rax") =
        static_cast<a9n::Sword>(a9n::KernelCallType::DebugCall);
    register unsigned long arg0 __asm__("rdi") = static_cast<unsigned char>(c);

    __asm__ volatile(
        "syscall"
        : "+a"(kernel_call_no)
        : "D"(arg0)
        : "rcx", "r11", "memory");
}

} // namespace a9n::abi
