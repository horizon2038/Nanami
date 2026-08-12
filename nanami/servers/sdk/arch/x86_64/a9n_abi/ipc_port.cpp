#include <a9n/abi/ipc_port.hpp>

namespace a9n::abi {

bool ipc_port_call(
    Word target_descriptor,
    Word* info,
    Word* identifier,
    Word message_registers[6]
) {
    if (!info || !identifier || !message_registers) {
        return false;
    }

    register Sword kernel_call_no __asm__("rax") =
        static_cast<Sword>(KernelCallType::CapabilityCall);
    register Word a0 __asm__("rdi") = target_descriptor;
    register Word a1 __asm__("rsi") = 3;
    register Word a2 __asm__("rdx") = *info;
    register Word a3 __asm__("r8") = *identifier;
    register Word a4 __asm__("r9") = message_registers[0];
    register Word a5 __asm__("r10") = message_registers[1];
    register Word a6 __asm__("r12") = message_registers[2];
    register Word a7 __asm__("r13") = message_registers[3];
    register Word a8 __asm__("r14") = message_registers[4];
    register Word a9 __asm__("r15") = message_registers[5];

    __asm__ volatile(
        "syscall"
        : "+a"(kernel_call_no), "+D"(a0), "+S"(a1), "+d"(a2),
          "+r"(a4), "+r"(a5), "+r"(a6), "+r"(a7), "+r"(a8), "+r"(a9),
          "=r"(a3)
        :
        : "rcx", "r11", "memory");

    *info = a2;
    *identifier = a3;
    message_registers[0] = a4;
    message_registers[1] = a5;
    message_registers[2] = a6;
    message_registers[3] = a7;
    message_registers[4] = a8;
    message_registers[5] = a9;
    return a0 != 0;
}

} // namespace a9n::abi
