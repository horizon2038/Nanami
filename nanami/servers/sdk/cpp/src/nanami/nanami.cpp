#include <a9n/abi/debug_call.hpp>
#include <a9n/types/types.hpp>
#include <nanami/nanami.hpp>

namespace {

constexpr a9n::Word OS_PORT_SLOT2_DESCRIPTOR = 0x0802000000000000ull;

constexpr a9n::Word OS_REQUEST_IRQ_CONTROL = 0x1001;
constexpr a9n::Word OS_REQUEST_IO_PORT_CONTROL = 0x1002;
constexpr a9n::Word OS_REQUEST_SERVICE_REGISTER = 0x1003;
constexpr a9n::Word OS_REQUEST_PAGE_ALLOC = 0x1004;
constexpr a9n::Word OS_REQUEST_DMA_REQUEST = 0x1006;
constexpr a9n::Word OS_REQUEST_MMIO_REQUEST = 0x1007;
constexpr a9n::Word OS_REQUEST_SHARED_MEMORY_CREATE = 0x1008;
constexpr a9n::Word OS_REQUEST_EXIT = 0x100a;
constexpr a9n::Word OS_REQUEST_DEBUG_PING = 0x10ff;

constexpr a9n::Word OS_RESPONSE_OK = 0;
constexpr a9n::Word OS_RESPONSE_INVALID_ARGUMENT = 1;
constexpr a9n::Word OS_SERVICE_NET_DEVICE = 1;
constexpr a9n::Word OS_SERVICE_PORT_SLOT_NET_DEVICE = 20;
constexpr a9n::Word OS_RESPONSE_PONG_MAGIC = 0x504f4e47;

inline a9n::Word make_message_info(bool is_block, unsigned message_length) {
    return (static_cast<a9n::Word>(is_block) << 0)
        | ((static_cast<a9n::Word>(message_length) & 0xFF) << 1)
        | (0ull << 13); // MessageSource::Normal
}

inline a9n::NanamiStatus map_status(a9n::Word status) {
    if (status == OS_RESPONSE_OK) {
        return a9n::NanamiStatus::Ok;
    }
    if (status == OS_RESPONSE_INVALID_ARGUMENT) {
        return a9n::NanamiStatus::InvalidArgument;
    }
    return a9n::NanamiStatus::Unsupported;
}

inline a9n::NanamiStatus call_port(
    a9n::Word target_descriptor,
    a9n::Word request_code,
    a9n::Word arg0,
    a9n::Word arg1,
    a9n::Word arg2,
    a9n::Word arg3,
    unsigned message_length,
    a9n::Word* out_status,
    a9n::Word* out_detail0,
    a9n::Word* out_detail1
) {
    register a9n::Sword kernel_call_no __asm__("rax") =
        static_cast<a9n::Sword>(a9n::KernelCallType::CapabilityCall);
    register a9n::Word a0 __asm__("rdi") = target_descriptor;
    register a9n::Word a1 __asm__("rsi") = 3; // ipc_port::OperationType::Call
    register a9n::Word a2 __asm__("rdx") = make_message_info(true, message_length);
    register a9n::Word a3 __asm__("r8") = 0;
    register a9n::Word a4 __asm__("r9") = request_code;
    register a9n::Word a5 __asm__("r10") = arg0;
    register a9n::Word a6 __asm__("r12") = arg1;
    register a9n::Word a7 __asm__("r13") = arg2;
    register a9n::Word a8 __asm__("r14") = arg3;
    register a9n::Word a9 __asm__("r15") = 0;

    __asm__ volatile(
        "syscall"
        : "+a"(kernel_call_no), "+D"(a0), "+S"(a1), "+d"(a2),
          "+r"(a4), "+r"(a5), "+r"(a6), "+r"(a7), "+r"(a8), "+r"(a9),
          "=r"(a3)
        :
        : "rcx", "r11", "memory");

    if (a0 == 0) {
        return a9n::NanamiStatus::Unsupported;
    }
    const a9n::Word source = (a2 >> 13) & 0x3;
    if (source != 0 || (((a2 >> 1) & 0xFF) < 3)) {
        return a9n::NanamiStatus::Unsupported;
    }

    if (out_status) {
        *out_status = a4;
    }
    if (out_detail0) {
        *out_detail0 = a5;
    }
    if (out_detail1) {
        *out_detail1 = a6;
    }

    return map_status(a4);
}

inline a9n::NanamiStatus os_call(
    a9n::Word request_code,
    a9n::Word arg0,
    a9n::Word arg1,
    a9n::Word arg2,
    a9n::Word arg3,
    unsigned message_length,
    a9n::Word* out_status,
    a9n::Word* out_detail0,
    a9n::Word* out_detail1
) {
    return call_port(
        OS_PORT_SLOT2_DESCRIPTOR,
        request_code,
        arg0,
        arg1,
        arg2,
        arg3,
        message_length,
        out_status,
        out_detail0,
        out_detail1
    );
}

} // namespace

namespace nanami {

void write_char(char c) {
    a9n::abi::debug_write_char(c);
}

void write_string(const char* s) {
    while (*s != 0) {
        write_char(*s++);
    }
}

a9n::NanamiStatus ping(a9n::Word token, a9n::Word* echoed_token) {
    if (!echoed_token) {
        return a9n::NanamiStatus::InvalidArgument;
    }

    a9n::Word status = 0;
    a9n::Word detail0 = 0;
    a9n::Word detail1 = 0;
    a9n::NanamiStatus rc = os_call(
        OS_REQUEST_DEBUG_PING,
        token,
        0,
        0,
        0,
        5,
        &status,
        &detail0,
        &detail1
    );
    if (rc != a9n::NanamiStatus::Ok) {
        return rc;
    }
    if (status != OS_RESPONSE_OK || detail1 != OS_RESPONSE_PONG_MAGIC) {
        return a9n::NanamiStatus::Unsupported;
    }

    *echoed_token = detail0;
    return a9n::NanamiStatus::Ok;
}

a9n::NanamiStatus register_service_net_device() {
    a9n::Word status = 0;
    a9n::NanamiStatus rc = os_call(
        OS_REQUEST_SERVICE_REGISTER,
        OS_SERVICE_NET_DEVICE,
        OS_SERVICE_PORT_SLOT_NET_DEVICE,
        0,
        0,
        4,
        &status,
        nullptr,
        nullptr
    );
    if (rc != a9n::NanamiStatus::Ok) {
        return rc;
    }
    return map_status(status);
}

a9n::NanamiStatus request_irq(a9n::Word irq_number, a9n::Word notification_slot, a9n::Word interrupt_slot) {
    a9n::Word status = 0;
    a9n::NanamiStatus rc = os_call(
        OS_REQUEST_IRQ_CONTROL,
        irq_number,
        notification_slot,
        interrupt_slot,
        0,
        4,
        &status,
        nullptr,
        nullptr
    );
    if (rc != a9n::NanamiStatus::Ok) {
        return rc;
    }
    return map_status(status);
}

a9n::NanamiStatus request_io_port(a9n::Word range_min, a9n::Word range_max, a9n::Word io_slot) {
    a9n::Word status = 0;
    a9n::NanamiStatus rc = os_call(
        OS_REQUEST_IO_PORT_CONTROL,
        range_min,
        range_max,
        io_slot,
        0,
        4,
        &status,
        nullptr,
        nullptr
    );
    if (rc != a9n::NanamiStatus::Ok) {
        return rc;
    }
    return map_status(status);
}

a9n::NanamiStatus net_device_send(
    a9n::Word device_port_descriptor,
    a9n::Word buffer_address,
    a9n::Word buffer_length,
    a9n::Word* transferred_length
) {
    a9n::Word status = 0;
    a9n::Word detail0 = 0;
    a9n::NanamiStatus rc = call_port(
        device_port_descriptor,
        NET_DEVICE_REQUEST_SEND,
        buffer_address,
        buffer_length,
        0,
        0,
        3,
        &status,
        &detail0,
        nullptr
    );
    if (rc != a9n::NanamiStatus::Ok) {
        return rc;
    }
    if (transferred_length) {
        *transferred_length = detail0;
    }
    return map_status(status);
}

a9n::NanamiStatus net_device_recv(
    a9n::Word device_port_descriptor,
    a9n::Word buffer_address,
    a9n::Word buffer_length,
    a9n::Word* received_length
) {
    a9n::Word status = 0;
    a9n::Word detail0 = 0;
    a9n::NanamiStatus rc = call_port(
        device_port_descriptor,
        NET_DEVICE_REQUEST_RECV,
        buffer_address,
        buffer_length,
        0,
        0,
        3,
        &status,
        &detail0,
        nullptr
    );
    if (rc != a9n::NanamiStatus::Ok) {
        return rc;
    }
    if (received_length) {
        *received_length = detail0;
    }
    return map_status(status);
}

a9n::NanamiStatus net_device_control(
    a9n::Word device_port_descriptor,
    a9n::Word control_code,
    a9n::Word arg0,
    a9n::Word arg1
) {
    a9n::Word status = 0;
    a9n::NanamiStatus rc = call_port(
        device_port_descriptor,
        NET_DEVICE_REQUEST_CONTROL,
        control_code,
        arg0,
        arg1,
        0,
        4,
        &status,
        nullptr,
        nullptr
    );
    if (rc != a9n::NanamiStatus::Ok) {
        return rc;
    }
    return map_status(status);
}

a9n::NanamiStatus request_pages(a9n::Word page_count) {
    if (page_count == 0) {
        return a9n::NanamiStatus::InvalidArgument;
    }

    a9n::Word status = 0;
    a9n::NanamiStatus rc = os_call(
        OS_REQUEST_PAGE_ALLOC,
        page_count,
        0,
        0,
        0,
        2,
        &status,
        nullptr,
        nullptr
    );
    if (rc != a9n::NanamiStatus::Ok) {
        return rc;
    }
    return map_status(status);
}

a9n::NanamiStatus request_exit() {
    a9n::Word status = 0;
    a9n::NanamiStatus rc = os_call(
        OS_REQUEST_EXIT,
        0,
        0,
        0,
        0,
        1,
        &status,
        nullptr,
        nullptr
    );
    if (rc != a9n::NanamiStatus::Ok) {
        return rc;
    }
    return map_status(status);
}

a9n::NanamiStatus request_dma(a9n::Word size_bytes, a9n::Word* out_paddr, a9n::Word* out_vaddr) {
    if (size_bytes == 0 || !out_paddr || !out_vaddr) {
        return a9n::NanamiStatus::InvalidArgument;
    }
    a9n::Word status = 0;
    a9n::Word detail0 = 0;
    a9n::Word detail1 = 0;
    a9n::NanamiStatus rc = os_call(
        OS_REQUEST_DMA_REQUEST,
        size_bytes,
        0,
        0,
        0,
        2,
        &status,
        &detail0,
        &detail1
    );
    if (rc != a9n::NanamiStatus::Ok) {
        return rc;
    }
    if (status != OS_RESPONSE_OK) {
        return map_status(status);
    }
    *out_paddr = detail0;
    *out_vaddr = detail1;
    return a9n::NanamiStatus::Ok;
}

a9n::NanamiStatus request_mmio(
    a9n::Word physical_address,
    a9n::Word size_bytes,
    a9n::Word* out_paddr,
    a9n::Word* out_vaddr
) {
    if (physical_address == 0 || size_bytes == 0 || !out_paddr || !out_vaddr) {
        return a9n::NanamiStatus::InvalidArgument;
    }
    a9n::Word status = 0;
    a9n::Word detail0 = 0;
    a9n::Word detail1 = 0;
    a9n::NanamiStatus rc = os_call(
        OS_REQUEST_MMIO_REQUEST,
        physical_address,
        size_bytes,
        0,
        0,
        3,
        &status,
        &detail0,
        &detail1
    );
    if (rc != a9n::NanamiStatus::Ok) {
        return rc;
    }
    if (status != OS_RESPONSE_OK) {
        return map_status(status);
    }
    *out_paddr = detail0;
    *out_vaddr = detail1;
    return a9n::NanamiStatus::Ok;
}

a9n::NanamiStatus request_shared_memory(
    a9n::Word peer_pid,
    a9n::Word size_bytes,
    a9n::Word* out_local_vaddr,
    a9n::Word* out_peer_vaddr
) {
    if (peer_pid == 0 || size_bytes == 0 || !out_local_vaddr || !out_peer_vaddr) {
        return a9n::NanamiStatus::InvalidArgument;
    }
    a9n::Word status = 0;
    a9n::Word detail0 = 0;
    a9n::Word detail1 = 0;
    a9n::NanamiStatus rc = os_call(
        OS_REQUEST_SHARED_MEMORY_CREATE,
        peer_pid,
        size_bytes,
        0,
        0,
        3,
        &status,
        &detail0,
        &detail1
    );
    if (rc != a9n::NanamiStatus::Ok) {
        return rc;
    }
    if (status != OS_RESPONSE_OK) {
        return map_status(status);
    }
    *out_local_vaddr = detail0;
    *out_peer_vaddr = detail1;
    return a9n::NanamiStatus::Ok;
}

} // namespace nanami
