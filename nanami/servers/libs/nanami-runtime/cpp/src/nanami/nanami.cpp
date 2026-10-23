#include <a9n/abi/debug_call.hpp>
#include <a9n/abi/ipc_port.hpp>
#include <a9n/types/types.hpp>
#include <nanami/nanami.hpp>

namespace {

constexpr a9n::Word OS_PORT_SLOT2_DESCRIPTOR = 0x0802000000000000ull;

constexpr a9n::Word OS_REQUEST_IRQ_CONTROL = 0x1001;
constexpr a9n::Word OS_REQUEST_IO_PORT_CONTROL = 0x1002;
constexpr a9n::Word OS_REQUEST_SERVICE_REGISTER = 0x1003;
constexpr a9n::Word OS_REQUEST_PAGE_ALLOC = 0x1004;
constexpr a9n::Word OS_REQUEST_SERVICE_CONNECT = 0x1005;
constexpr a9n::Word OS_REQUEST_DMA_REQUEST = 0x1006;
constexpr a9n::Word OS_REQUEST_MMIO_REQUEST = 0x1007;
constexpr a9n::Word OS_REQUEST_SHARED_MEMORY_CREATE = 0x1008;
constexpr a9n::Word OS_REQUEST_EXIT = 0x100a;
constexpr a9n::Word OS_REQUEST_DEBUG_PING = 0x10ff;
constexpr a9n::Word OS_REQUEST_SESSION_CONNECT = 0x1027;
constexpr a9n::Word OS_REQUEST_BINDING_CONTROL = 0x1028;

constexpr a9n::Word OS_RESPONSE_OK = 0;
constexpr a9n::Word OS_RESPONSE_INVALID_ARGUMENT = 1;
constexpr a9n::Word OS_RESPONSE_PONG_MAGIC = 0x504f4e47;

inline a9n::NanamiStatus map_status(a9n::Word status) {
    if (status == OS_RESPONSE_OK) {
        return a9n::NanamiStatus::Ok;
    }
    if (status == OS_RESPONSE_INVALID_ARGUMENT) {
        return a9n::NanamiStatus::InvalidArgument;
    }
    return a9n::NanamiStatus::Unsupported;
}

bool pack_service_name(const char* name, a9n::Word words[3]) {
    if (!name || name[0] == 0) {
        return false;
    }
    for (unsigned index = 0; name[index] != 0; ++index) {
        if (index >= 24) {
            return false;
        }
        words[index / 8] |= static_cast<a9n::Word>(static_cast<unsigned char>(name[index]))
            << ((index % 8) * 8);
    }
    return true;
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
    a9n::Word info = a9n::abi::make_normal_message_info(true, message_length);
    a9n::Word identifier = 0;
    a9n::Word message_registers[6] = {request_code, arg0, arg1, arg2, arg3, 0};
    if (!a9n::abi::ipc_port_call(
            target_descriptor,
            &info,
            &identifier,
            message_registers
        )) {
        return a9n::NanamiStatus::Unsupported;
    }
    if (!a9n::abi::is_normal_message(info) || a9n::abi::message_length(info) < 3) {
        return a9n::NanamiStatus::Unsupported;
    }

    if (out_status) {
        *out_status = message_registers[0];
    }
    if (out_detail0) {
        *out_detail0 = message_registers[1];
    }
    if (out_detail1) {
        *out_detail1 = message_registers[2];
    }

    return map_status(message_registers[0]);
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

a9n::NanamiStatus register_service(const char* name, a9n::Word service_slot) {
    a9n::Word words[3] = {};
    if (!pack_service_name(name, words)) {
        return a9n::NanamiStatus::InvalidArgument;
    }
    a9n::Word status = 0;
    a9n::NanamiStatus rc = os_call(
        OS_REQUEST_SERVICE_REGISTER,
        words[0],
        words[1],
        words[2],
        service_slot,
        5,
        &status,
        nullptr,
        nullptr
    );
    if (rc != a9n::NanamiStatus::Ok) {
        return rc;
    }
    return map_status(status);
}

a9n::NanamiStatus connect_service(const char* name, a9n::Word destination_slot, a9n::Word* owner_pid) {
    a9n::Word words[3] = {};
    if (!pack_service_name(name, words)) {
        return a9n::NanamiStatus::InvalidArgument;
    }
    a9n::Word owner = 0;
    const a9n::NanamiStatus result = os_call(
        OS_REQUEST_SERVICE_CONNECT, destination_slot, words[0], words[1], words[2],
        5, nullptr, &owner, nullptr
    );
    if (result == a9n::NanamiStatus::Ok && owner_pid) {
        *owner_pid = owner;
    }
    return result;
}

a9n::NanamiStatus register_service_net_device() {
    return register_service("net-device", 20);
}

a9n::NanamiStatus connect_managed_service(
    const char* name, a9n::Word destination_slot, ServiceConnection* connection
) {
    a9n::Word words[3] = {};
    if (!connection || !pack_service_name(name, words)) {
        return a9n::NanamiStatus::InvalidArgument;
    }
    a9n::Word session = 0;
    a9n::Word service = 0;
    const a9n::NanamiStatus result = os_call(
        OS_REQUEST_SESSION_CONNECT, destination_slot, words[0], words[1], words[2],
        5, nullptr, &session, &service
    );
    if (result == a9n::NanamiStatus::Ok) {
        *connection = {0x0800000000000000ull | ((destination_slot & 0xff) << 48), session, service};
    }
    return result;
}

a9n::NanamiStatus close_managed_session(a9n::Word session) {
    constexpr a9n::Word close_operation = 9;
    return os_call(OS_REQUEST_BINDING_CONTROL, close_operation, session, 0, 0, 5, nullptr, nullptr, nullptr);
}

a9n::NanamiStatus create_session_buffer(a9n::Word session, a9n::Word bytes, SessionBuffer* buffer) {
    if (!buffer || bytes == 0 || bytes > 65536) {
        return a9n::NanamiStatus::InvalidArgument;
    }
    a9n::Word address = 0;
    a9n::Word mapped_bytes = 0;
    const a9n::NanamiStatus result = os_call(OS_REQUEST_BINDING_CONTROL, 11, session, bytes, 0, 5, nullptr, &address, &mapped_bytes);
    if (result == a9n::NanamiStatus::Ok) {
        *buffer = {address, mapped_bytes};
    }
    return result;
}

a9n::NanamiStatus session_buffer(a9n::Word session, SessionBuffer* buffer) {
    if (!buffer) {
        return a9n::NanamiStatus::InvalidArgument;
    }
    a9n::Word address = 0;
    a9n::Word bytes = 0;
    const a9n::NanamiStatus result = os_call(OS_REQUEST_BINDING_CONTROL, 12, session, 0, 0, 5, nullptr, &address, &bytes);
    if (result == a9n::NanamiStatus::Ok) {
        *buffer = {address, bytes};
    }
    return result;
}

a9n::NanamiStatus connect_session_event(a9n::Word session, a9n::Word destination_slot) {
    if (destination_slot < 23 || destination_slot >= 224) {
        return a9n::NanamiStatus::InvalidArgument;
    }
    return os_call(OS_REQUEST_BINDING_CONTROL, 13, session, destination_slot, 0, 5, nullptr, nullptr, nullptr);
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
        0x20, // Attribute::UNCACHED; ordinary MMIO is never WB/WC.
        0,
        4, // code + paddr + bytes + mapping
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
