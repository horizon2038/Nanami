#ifndef A9N_ABI_IPC_PORT_HPP
#define A9N_ABI_IPC_PORT_HPP

#include <a9n/types/types.hpp>

namespace a9n::abi {

constexpr Word make_normal_message_info(bool is_block, unsigned length) {
    return static_cast<Word>(is_block)
        | ((static_cast<Word>(length) & 0xFF) << 1);
}

constexpr unsigned message_length(Word info) {
    return static_cast<unsigned>((info >> 1) & 0xFF);
}

constexpr bool is_normal_message(Word info) {
    return ((info >> 13) & 0x3) == 0;
}

bool ipc_port_call(
    Word target_descriptor,
    Word* info,
    Word* identifier,
    Word message_registers[6]
);

} // namespace a9n::abi

#endif
