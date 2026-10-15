// Host-only inspection harness. Use production methods, not a scheduler model.
// Access modifiers are relaxed solely to prepare exact blocked/ready fixtures.
#define private public
#include <kernel/capability/notification_port.hpp>
#include <kernel/process/cpu.hpp>
#undef private
#include <kernel/utility/logger.hpp>
#include <stdio.h>

using namespace a9n::kernel;

static cpu_local_variable *active;
static unsigned switches;
static unsigned unexpected_hal_calls;

// Avoid retaining unused virtual dispatch paths in this minimal host fixture.
struct endpoint final : ipc_port {
    capability_result execute(process &, capability_slot &) override { return {}; }
};
struct notification_endpoint final : notification_port {
    capability_result execute(process &, capability_slot &) override { return {}; }
};

namespace a9n::hal {
liba9n::result<cpu_local_variable *, hal_error> current_local_variable() { return active; }
hal_result switch_context(process &, process &) { ++switches; return {}; }
hal_result send_ipi(ipi_type, a9n::word) { ++unexpected_hal_calls; return hal_error::UNEXPECTED; }
liba9n::result<a9n::word, hal_error> get_general_register(const process &, register_type) {
    ++unexpected_hal_calls;
    return hal_error::UNEXPECTED;
}
bool is_valid_user_address(a9n::virtual_address) {
    ++unexpected_hal_calls;
    return false;
}
liba9n::result<a9n::word, hal_error> get_message_register(const process &p, a9n::word index) {
    return static_cast<a9n::word>(p.registers[index]);
}
hal_result configure_message_register(process &p, a9n::word index, a9n::word value) {
    p.registers[index] = value;
    return {};
}
}
namespace a9n::kernel {
liba9n::result<process *, kernel_error> current_process_on_this_core() { return active->current_process; }
kernel_result reschedule_core(a9n::word) { return kernel_error::UNEXPECTED; }
}
namespace a9n::kernel::utility {
void logger::printk(const char *, ...) {}
void logger::printh(const char *, ...) {}
void logger::error(const char *) {}
}

#define REQUIRE(condition) do { if (!(condition)) { \
    fprintf(stderr, "FAIL %s:%d: %s\n", __FILE__, __LINE__, #condition); return 1; \
} } while (0)

int main(int argc, char **argv) {
    if (argc < 2 || argc > 3) return 2;
    active = &cpu_local_variables[BSP_ID];
    auto &manager = active->process_manager_core;
    auto &idle = manager.idle_process;
    idle.status = process_status::IDLE;
    idle.priority = 0;
    idle.quantum = QUANTUM_MAX;
    idle.core_affinity = BSP_ID;
    active->core_number = BSP_ID;
    active->current_process = &idle;
    active->is_idle = true;

    const bool reply_case = argv[1][0] == 'r';
    const bool bound = argv[1][0] == 'b';
    const bool wait = argv[1][0] == 'w';
    if (!bound && !wait && !reply_case) return 2;
    const bool busy_case = reply_case || argv[1][bound ? 6 : 5] == 'b';
    process busy {};
    if (busy_case) {
        busy.status = process_status::READY;
        busy.priority = 28;
        busy.quantum = 7;
        busy.core_affinity = BSP_ID;
        active->current_process = &busy;
        active->is_idle = false;
    }

    process worker {};
    worker.priority = 30;
    worker.quantum = QUANTUM_MAX;
    worker.core_affinity = BSP_ID;
    notification_endpoint notification;
    capability_slot signal {};
    REQUIRE(try_configure_notification_port_slot(signal, notification, 1));

    endpoint ipc;
    if (reply_case) {
        busy.priority = 30; // Timer.
        worker.priority = 28; // USB caller.
        worker.status = process_status::BLOCKED_REPLY;
        worker.source_reply_state = process::source_reply_state_object::WAIT;
        worker.source_reply_target = &busy;
        busy.destination_reply_state = process::destination_reply_state_object::READY_TO_REPLY;
        busy.destination_reply_target = &worker;
        busy.registers[4] = 0;
        busy.registers[5] = 10;
        busy.registers[6] = 0;
        capability_slot service {};
        REQUIRE(try_configure_ipc_port_slot(service, ipc, 6));
        using info = ipc_port::message_info;
        REQUIRE(ipc.operation_reply(busy, service, info(false, 3, 0, info::message_source::NORMAL)));
        REQUIRE(active->current_process == &busy);
        REQUIRE(worker.status == process_status::READY && worker.is_in_ready_queue);
        REQUIRE(worker.source_reply_state == process::source_reply_state_object::NONE);
        REQUIRE(busy.destination_reply_state == process::destination_reply_state_object::NONE);
        REQUIRE(worker.registers[4] == 0 && worker.registers[5] == 10 && worker.registers[6] == 0);

        if (argv[1][6] == 'p') { // reply-pending: a Timer IRQ precedes Receive.
            REQUIRE(notification.bind_process(busy));
            REQUIRE(try_configure_notification_port_slot(busy.binded_notification_port, notification, 1));
            REQUIRE(notification.operation_notify(busy, signal));
            REQUIRE(ipc.operation_receive(busy, service, info(true, 0, 0, info::message_source::NORMAL)));
            REQUIRE(active->current_process == &busy);
            REQUIRE(info(busy.registers[2]).is_notification());
            REQUIRE(worker.is_in_ready_queue);
            // Periodic notification to an already-READY USB must not overwrite
            // the normal Call reply waiting in its message registers.
            notification_endpoint client_notification;
            capability_slot interval_signal {};
            REQUIRE(client_notification.bind_process(worker));
            REQUIRE(try_configure_notification_port_slot(interval_signal, client_notification, 1ul << 63));
            REQUIRE(client_notification.operation_notify(busy, interval_signal));
            REQUIRE(info(worker.registers[2]).is_normal());
            REQUIRE(worker.registers[4] == 0 && worker.registers[5] == 10);
        }
        REQUIRE(ipc.operation_receive(busy, service, info(true, 0, 0, info::message_source::NORMAL)));
        REQUIRE(active->current_process == &worker && !active->is_idle);
        REQUIRE(busy.status == process_status::BLOCKED_RECEIVE);
        REQUIRE(!worker.is_in_ready_queue);
        REQUIRE(unexpected_hal_calls == 0);
        printf("PASS: %s dispatched USB with its original normal reply\n", argv[1]);
        return 0;
    }

    if (argc == 3) worker.priority = argv[2][0] == 'e' ? busy.priority : busy.priority - 1;
    if (bound) {
        worker.status = process_status::BLOCKED_RECEIVE;
        worker.current_ipc_port = &ipc;
        ipc.queue_head = ipc.queue_tail = &worker;
        ipc.state = ipc_port::READY_TO_RECEIVE;
        REQUIRE(notification.bind_process(worker));
    } else {
        worker.status = process_status::BLOCKED_WAIT;
        worker.current_notification_port = &notification;
        notification.queue_head = notification.queue_tail = &worker;
        notification.state = notification_port::READY_TO_WAKE;
    }

    REQUIRE(notification.operation_notify(*active->current_process, signal));
    REQUIRE(worker.status == process_status::READY);
    printf("%s: worker-ready=%d worker-queued=%d cpu-idle=%d switches=%u\n",
           argv[1], worker.status == process_status::READY, worker.is_in_ready_queue,
           active->is_idle, switches);
    if (argc == 3) {
        REQUIRE(active->current_process == &busy && switches == 0);
        REQUIRE(worker.is_in_ready_queue);
        REQUIRE(unexpected_hal_calls == 0);
        puts("PASS: equal/lower priority wakeup did not preempt the current worker");
        return 0;
    }
    // An idle CPU should dispatch work on wakeup, not wait for another IRQ.
    REQUIRE(active->current_process == &worker);
    REQUIRE(!active->is_idle);
    REQUIRE(switches == 1);
    REQUIRE(unexpected_hal_calls == 0);
    puts("PASS: notification dispatches a ready higher-priority worker");
    return 0;
}
