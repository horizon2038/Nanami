// Compile with the production C++ runtime; only kernel IPC/debug effects are substituted.
#include <a9n/abi/ipc_port.hpp>
#include <nanami/nanami.hpp>
#include <array>
#include <cassert>
#include <cstring>

static std::array<a9n::Word, 5> request;
static unsigned calls;
static a9n::Word response_status;
static bool transport_ok = true;

namespace a9n::abi {
void debug_write_char(char) {}
bool ipc_port_call(Word port, Word* info, Word*, Word registers[6]) {
    assert(port == 0x0802000000000000ull);
    assert(message_length(*info) == 5);
    ++calls;
    for (unsigned index = 0; index < request.size(); ++index) {
        request[index] = registers[index];
    }
    *info = make_normal_message_info(true, 3);
    registers[0] = response_status;
    registers[1] = 77;
    registers[2] = 0;
    return transport_ok;
}
}

static void check_name(const char* expected, unsigned first) {
    std::array<unsigned char, 24> name = {};
    for (unsigned index = 0; index < name.size(); ++index) {
        name[index] = (request[first + index / 8] >> ((index % 8) * 8)) & 0xff;
    }
    assert(std::memcmp(name.data(), expected, std::strlen(expected)) == 0);
    for (unsigned index = std::strlen(expected); index < name.size(); ++index) {
        assert(name[index] == 0);
    }
}

int main() {
    const char* arbitrary = "abcdefghijklmnopqrstuvwx";
    assert(nanami::register_service(arbitrary, 27) == a9n::NanamiStatus::Ok);
    assert(request[0] == 0x1003 && request[4] == 27);
    check_name(arbitrary, 1);
    a9n::Word owner = 0;
    assert(nanami::connect_service("unfamiliar-42", 28, &owner) == a9n::NanamiStatus::Ok);
    assert(request[0] == 0x1005 && request[1] == 28 && owner == 77);
    check_name("unfamiliar-42", 2);
    assert(nanami::register_service_net_device() == a9n::NanamiStatus::Ok);
    check_name("net-device", 1);
    assert(request[4] == 20);
    const unsigned before = calls;
    const char* invalid_names[] = {nullptr, "", "abcdefghijklmnopqrstuvwxy"};
    for (const char* invalid : invalid_names) {
        assert(nanami::register_service(invalid, 20) == a9n::NanamiStatus::InvalidArgument);
        assert(nanami::connect_service(invalid, 22) == a9n::NanamiStatus::InvalidArgument);
    }
    assert(calls == before);
    nanami::ServiceConnection connection = {};
    assert(nanami::connect_managed_service("arbitrary-managed", 30, &connection) == a9n::NanamiStatus::Ok);
    assert(request[0] == 0x1027 && request[1] == 30);
    check_name("arbitrary-managed", 2);
    assert(connection.endpoint == (0x0800000000000000ull | (30ull << 48)));
    assert(connection.session == 77 && connection.service == 0);
    assert(nanami::close_managed_session(77) == a9n::NanamiStatus::Ok);
    assert(request[0] == 0x1028 && request[1] == 9 && request[2] == 77);
    nanami::SessionBuffer buffer = {};
    assert(nanami::create_session_buffer(77, 65536, &buffer) == a9n::NanamiStatus::Ok);
    assert(request[0] == 0x1028 && request[1] == 11 && request[2] == 77 && request[3] == 65536);
    assert(buffer.address == 77);
    assert(nanami::session_buffer(77, &buffer) == a9n::NanamiStatus::Ok);
    assert(request[1] == 12 && request[2] == 77 && request[3] == 0);
    assert(nanami::create_session_buffer(77, 65537, &buffer) == a9n::NanamiStatus::InvalidArgument);
    assert(nanami::session_buffer(77, nullptr) == a9n::NanamiStatus::InvalidArgument);
    assert(nanami::connect_session_event(77, 32) == a9n::NanamiStatus::Ok);
    assert(request[0] == 0x1028 && request[1] == 13 && request[2] == 77 && request[3] == 32);
    assert(nanami::connect_session_event(77, 224) == a9n::NanamiStatus::InvalidArgument);
    response_status = 2;
    owner = 99;
    assert(nanami::connect_service("denied", 22, &owner) == a9n::NanamiStatus::Unsupported);
    assert(owner == 99);
    connection.service = 99;
    assert(nanami::connect_managed_service("denied", 31, &connection) == a9n::NanamiStatus::Unsupported);
    assert(connection.service == 99);
    buffer.address = 99;
    assert(nanami::create_session_buffer(77, 4096, &buffer) == a9n::NanamiStatus::Unsupported);
    assert(buffer.address == 99);
    transport_ok = false;
    assert(nanami::register_service("transport-error", 20) == a9n::NanamiStatus::Unsupported);
}
