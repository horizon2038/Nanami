#ifndef NANAMI_SDK_NANAMI_HPP
#define NANAMI_SDK_NANAMI_HPP

#include <a9n/types/types.hpp>

namespace nanami {

void write_char(char c);
void write_string(const char* s);
a9n::NanamiStatus request_pages(a9n::Word page_count);
a9n::NanamiStatus request_exit();
a9n::NanamiStatus request_dma(a9n::Word size_bytes, a9n::Word* out_paddr, a9n::Word* out_vaddr);
a9n::NanamiStatus request_mmio(
    a9n::Word physical_address,
    a9n::Word size_bytes,
    a9n::Word* out_paddr,
    a9n::Word* out_vaddr
);
a9n::NanamiStatus request_shared_memory(
    a9n::Word peer_pid,
    a9n::Word size_bytes,
    a9n::Word* out_local_vaddr,
    a9n::Word* out_peer_vaddr
);
a9n::NanamiStatus ping(a9n::Word token, a9n::Word* echoed_token);
// Names are 1..24 UTF-8 bytes. Returned owner PIDs are local to this Alpha.
a9n::NanamiStatus register_service(const char* name, a9n::Word service_slot);
a9n::NanamiStatus connect_service(
    const char* name,
    a9n::Word destination_slot,
    a9n::Word* owner_pid = nullptr
);
a9n::NanamiStatus register_service_net_device();
struct ServiceConnection {
    a9n::Word endpoint;
    // Local Alpha close handle; an imported upstream Session badge may differ.
    a9n::Word session;
    a9n::Word service;
};
struct SessionBuffer {
    a9n::Word address;
    a9n::Word bytes;
};
a9n::NanamiStatus create_session_buffer(a9n::Word session, a9n::Word bytes, SessionBuffer* buffer);
a9n::NanamiStatus session_buffer(a9n::Word session, SessionBuffer* buffer);
a9n::NanamiStatus connect_session_event(a9n::Word session, a9n::Word destination_slot);
// Managed stateless-control Services use opaque local Session/Service identities.
a9n::NanamiStatus connect_managed_service(
    const char* name, a9n::Word destination_slot, ServiceConnection* connection
);
// Withdraws access; the current kernel requires retaining its alias/quota.
a9n::NanamiStatus close_managed_session(a9n::Word session);
a9n::NanamiStatus request_irq(a9n::Word irq_number, a9n::Word notification_slot, a9n::Word interrupt_slot);
a9n::NanamiStatus request_io_port(a9n::Word range_min, a9n::Word range_max, a9n::Word io_slot);

a9n::NanamiStatus net_device_send(
    a9n::Word device_port_descriptor,
    a9n::Word buffer_address,
    a9n::Word buffer_length,
    a9n::Word* transferred_length
);
a9n::NanamiStatus net_device_recv(
    a9n::Word device_port_descriptor,
    a9n::Word buffer_address,
    a9n::Word buffer_length,
    a9n::Word* received_length
);
a9n::NanamiStatus net_device_control(
    a9n::Word device_port_descriptor,
    a9n::Word control_code,
    a9n::Word arg0,
    a9n::Word arg1
);

constexpr a9n::Word NET_DEVICE_REQUEST_SEND = 0x2001;
constexpr a9n::Word NET_DEVICE_REQUEST_RECV = 0x2002;
constexpr a9n::Word NET_DEVICE_REQUEST_CONTROL = 0x2003;

constexpr a9n::Word NET_DEVICE_CONTROL_LINK_UP = 1;
constexpr a9n::Word NET_DEVICE_CONTROL_LINK_DOWN = 2;
constexpr a9n::Word NET_DEVICE_CONTROL_ATTACH_SHARED_MEMORY = 16;
constexpr a9n::Word NET_DEVICE_CONTROL_GET_MAC = 17;

} // namespace nanami

#endif
