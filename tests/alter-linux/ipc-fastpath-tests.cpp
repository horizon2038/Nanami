// Link the production scheduler, process manager, IPC and notification ports.
// Only HAL effects are substituted on the host.
#include <kernel/capability/capability_utility.hpp>
#include <kernel/capability/ipc_port.hpp>
#include <kernel/capability/notification_port.hpp>
#include <kernel/process/cpu.hpp>
#include <kernel/utility/logger.hpp>
#include <stdio.h>
#include <stdlib.h>

using namespace a9n::kernel;
#define CHECK(expr)                                                            \
  do {                                                                         \
    if (!(expr)) {                                                             \
      fprintf(stderr, "%s:%d: %s\n", __FILE__, __LINE__, #expr);               \
      abort();                                                                 \
    }                                                                          \
  } while (0)

static unsigned switches, ipis;
static a9n::word current_core;

namespace a9n::hal {
hal_result switch_context(process &, process &next) {
  CHECK(!next.is_in_ready_queue);
  CHECK(next.next == nullptr);
  CHECK(next.preview == nullptr);
  ++switches;
  return {};
}
liba9n::result<cpu_local_variable *, hal_error> current_local_variable() {
  return &cpu_local_variables[current_core];
}
hal_result send_ipi(ipi_type, a9n::word) {
  ++ipis;
  return {};
}
liba9n::result<a9n::word, hal_error> get_message_register(const process &p,
                                                          a9n::word i) {
  if (i < 10)
    return static_cast<a9n::word>(p.registers[i]);
  if (!p.buffer)
    return hal_error::NO_SUCH_ADDRESS;
  return p.buffer->get_message(i);
}
hal_result configure_message_register(process &p, a9n::word i,
                                      a9n::word value) {
  if (i < 10) {
    p.registers[i] = value;
    return {};
  }
  if (!p.buffer)
    return hal_error::NO_SUCH_ADDRESS;
  p.buffer->set_message(i, value);
  return {};
}
liba9n::result<a9n::word, hal_error> get_general_register(const process &,
                                                          register_type) {
  return a9n::word{0};
}
} // namespace a9n::hal
namespace a9n::kernel::utility {
void logger::error(const char *message) {
  fprintf(stderr, "kernel error: %s\n", message);
  abort();
}
void logger::printk(const char *, ...) { abort(); }
} // namespace a9n::kernel::utility

static process ready(a9n::sword priority) {
  process p{};
  p.status = process_status::READY;
  p.priority = priority;
  p.quantum = QUANTUM_MAX;
  return p;
}

static void scheduler_tests() {
  scheduler s{};
  auto low = ready(0), high = ready(PRIORITY_MAX - 1), peer = ready(PRIORITY_MAX - 1);
  CHECK(!s.schedule());
  CHECK(s.add_process(&low));
  CHECK(s.add_process(&high));
  CHECK(s.add_process(&peer));
  CHECK(!s.add_process(&high));
  CHECK(s.schedule().unwrap() == &high);
  CHECK(s.remove_process(&peer));
  CHECK(s.schedule().unwrap() == &low);
  CHECK(!s.schedule());
  CHECK(s.add_process(&high));
  CHECK(s.add_process(&peer));
  CHECK(s.remove_process(&high)); // peer remains queued
  CHECK(s.schedule().unwrap() == &peer);
  CHECK(!s.schedule());
  CHECK(!s.remove_process(&peer));
  low.priority = -1;
  CHECK(!s.add_process(&low));
  low.priority = PRIORITY_MAX;
  CHECK(!s.add_process(&low));
  low.priority = 0;
  low.next = &high;
  CHECK(!s.add_process(&low));

  // Deterministic randomized FIFO/priority oracle, including direct switches.
  scheduler random_scheduler{};
  process tasks[40]{};
  unsigned ages[40]{}, sequence = 0, rng = 1;
  bool queued[40]{};
  for (int i = 0; i < 40; ++i)
    tasks[i] = ready((i * 13) % PRIORITY_MAX);
  for (int step = 0; step < 20000; ++step) {
    rng = rng * 1664525u + 1013904223u;
    unsigned index = (rng >> 8) % 40;
    if ((rng & 3) == 0) {
      CHECK(bool(random_scheduler.remove_process(&tasks[index])) ==
            queued[index]);
      queued[index] = false;
    } else if ((rng & 3) == 1) {
      CHECK(bool(random_scheduler.add_process(&tasks[index])) ==
            !queued[index]);
      if (!queued[index])
        ages[index] = ++sequence;
      queued[index] = true;
    } else {
      int best = -1;
      for (int i = 0; i < 40; ++i)
        if (queued[i] &&
            (best < 0 || tasks[i].priority > tasks[best].priority ||
             (tasks[i].priority == tasks[best].priority &&
              ages[i] < ages[best])))
          best = i;
      const bool direct = (rng & 3) == 3 && !queued[index];
      if (direct) {
        if (best >= 0 && tasks[best].priority > tasks[index].priority) {
          queued[index] = true;
          ages[index] = ++sequence;
        } else {
          best = index;
        }
      }
      auto next = direct ? random_scheduler.try_direct_schedule(&tasks[index])
                         : random_scheduler.schedule();
      CHECK(bool(next) == (best >= 0));
      if (best >= 0) {
        CHECK(next.unwrap() == &tasks[best]);
        queued[best] = false;
      }
    }
    for (int i = 0; i < 40; ++i) {
      CHECK(tasks[i].is_in_ready_queue == queued[i]);
      if (!queued[i]) {
        CHECK(tasks[i].next == nullptr);
        CHECK(tasks[i].preview == nullptr);
      }
    }
  }
  { // A stale bound from a dequeued server must not force a FIFO fallback.
    scheduler direct;
    auto client = ready(0), server = ready(PRIORITY_MAX - 1), peer = ready(0);
    CHECK(direct.add_process(&server));
    CHECK(direct.add_process(&peer));
    CHECK(direct.schedule().unwrap() == &server);
    CHECK(direct.try_direct_schedule(&client).unwrap() == &client);
    CHECK(peer.is_in_ready_queue);
    // Directly selecting a server must not raise the ready-queue bound either.
    CHECK(direct.try_direct_schedule(&server).unwrap() == &server);
    CHECK(direct.try_direct_schedule(&client).unwrap() == &client);
    CHECK(direct.schedule().unwrap() == &peer);
  }
  for (a9n::sword priority = 0; priority < PRIORITY_MAX; ++priority) {
    scheduler direct;
    auto client = ready(PRIORITY_MAX / 2), other = ready(priority);
    CHECK(direct.add_process(&other));
    const bool higher = priority > client.priority;
    CHECK(direct.try_direct_schedule(&client).unwrap() ==
          (higher ? &other : &client));
    CHECK(client.is_in_ready_queue == higher);
    CHECK(other.is_in_ready_queue == !higher);
    CHECK(direct.schedule().unwrap() == (higher ? &client : &other));
    CHECK(!direct.schedule());
  }
}

struct conversation {
  process client = ready(3), server = ready(PRIORITY_MAX - 1);
  ipc_port port;
  capability_slot slot{};

  conversation() {
    current_core = 0;
    cpu_local_variables[0] = {};
    cpu_local_variables[1] = {};
    cpu_local_variables[0].current_process = &server;
    switches = ipis = 0;
    slot.rights = capability_slot::ALL;
    slot.data[0] = 0x1234;
    CHECK(mark_scheduled(server, client));
    CHECK(invoke(server, 2)); // receive, then run client
    call();
    CHECK(cpu_local_variables[0].current_process == &server);
    CHECK(client.status == process_status::BLOCKED_REPLY);
  }
  capability_result invoke(process &owner, unsigned operation,
                           a9n::word info = 1) {
    owner.registers[1] = operation;
    owner.registers[2] = info;
    return port.execute(owner, slot);
  }
  void call() { CHECK(invoke(client, 3)); }
  void reply(a9n::word info = 1) { CHECK(invoke(server, 5, info)); }
  void check_replied() {
    CHECK(client.status == process_status::READY);
    CHECK(client.source_reply_state ==
          process::source_reply_state_object::NONE);
    CHECK(client.source_reply_target == nullptr);
    CHECK(server.destination_reply_state ==
          process::destination_reply_state_object::NONE);
    CHECK(server.destination_reply_target == nullptr);
    CHECK(client.quantum == QUANTUM_MAX);
  }
};

// Minimal CSpace endpoints for exercising the production capability-transfer path.
struct test_node : capability_component {
  capability_slot *slot;
  explicit test_node(capability_slot &target) : slot(&target) {}
  capability_result execute(process &, capability_slot &) override { abort(); }
  capability_result revoke(capability_slot &) override { abort(); }
  capability_lookup_result retrieve_slot(a9n::word) override { return slot; }
  capability_lookup_result traverse_slot(a9n::capability_descriptor, a9n::word,
                                         a9n::word) override { return slot; }
};

static void ipc_tests() {
  { // Repeated round trips, including same priority and register payloads.
    conversation c;
    for (int i = 0; i < 1000; ++i) {
      c.server.quantum = 777;
      c.client.quantum = 2;
      c.server.registers[4] = 0xfeed;
      c.server.registers[9] = 0xbeef;
      c.reply(i & 1 ? 1 | (6 << 1) : 1);
      c.check_replied();
      CHECK(cpu_local_variables[0].current_process == &c.client);
      CHECK(!c.client.is_in_ready_queue);
      CHECK(c.server.status == process_status::BLOCKED_RECEIVE);
      CHECK(c.server.current_ipc_port == &c.port);
      CHECK(c.server.preview_ipc_queue == nullptr);
      CHECK(c.server.next_ipc_queue == nullptr);
      if (i & 1) {
        CHECK(c.client.registers[4] == 0xfeed);
        CHECK(c.client.registers[9] == 0xbeef);
      }
      CHECK(!ipis);
      c.server.priority = i & 1 ? c.client.priority : PRIORITY_MAX - 1;
      c.call();
      CHECK(c.server.current_ipc_port == nullptr);
      CHECK(c.server.preview_ipc_queue == nullptr);
      CHECK(c.server.next_ipc_queue == nullptr);
    }
  }
  for (int priority = 2; priority <= 4; ++priority) {
    conversation c;
    auto other = ready(priority);
    c.client.quantum = 2;
    // Work may become ready while the server handles the call.
    CHECK(mark_scheduled(c.server, other));
    other.quantum = 7;
    c.reply();
    c.check_replied();
    // Only a strictly higher-priority queued process prevents direct return.
    CHECK(cpu_local_variables[0].current_process ==
          (priority > 3 ? &other : &c.client));
    CHECK(c.client.is_in_ready_queue == (priority > 3));
    CHECK(other.is_in_ready_queue == (priority <= 3));
    CHECK(other.quantum == 7); // Replenish the client, not a fallback selection.
    CHECK(cpu_local_variables[0].process_manager_core.yield());
    CHECK(cpu_local_variables[0].current_process ==
          (priority < 3 ? &c.client : &other));
    CHECK(c.client.is_in_ready_queue == (priority >= 3));
    CHECK(other.is_in_ready_queue == (priority < 3));
  }
  { // A higher-priority peer can already be queued when the next call starts.
    conversation c;
    c.reply();
    auto other = ready(4);
    CHECK(mark_scheduled(c.client, other));
    c.call();
    CHECK(cpu_local_variables[0].current_process == &c.server);
    c.reply();
    c.check_replied();
    CHECK(cpu_local_variables[0].current_process == &other);
    CHECK(c.client.is_in_ready_queue);
  }
  { // A high-priority peer that stopped being ready must not block fast return.
    conversation c;
    auto other = ready(PRIORITY_MAX - 1), peer = ready(c.client.priority);
    CHECK(mark_scheduled(c.server, other));
    CHECK(mark_scheduled(c.server, peer));
    c.reply();
    CHECK(cpu_local_variables[0].current_process == &other);
    ipc_port wait_port;
    other.registers[1] = 2;
    other.registers[2] = 1;
    CHECK(wait_port.execute(other, c.slot));
    CHECK(cpu_local_variables[0].current_process == &peer);
    CHECK(cpu_local_variables[0].process_manager_core.yield());
    CHECK(cpu_local_variables[0].current_process == &c.client);
    c.call();
    c.reply();
    c.check_replied();
    CHECK(cpu_local_variables[0].current_process == &c.client);
    CHECK(peer.is_in_ready_queue);
  }
  { // Buffered payloads use the same validated transfer before committing.
    conversation c;
    ipc_buffer source{}, destination{};
    c.server.buffer = &source;
    c.client.buffer = &destination;
    for (a9n::word i = 4; i < 259; ++i)
      CHECK(a9n::hal::configure_message_register(c.server, i, i ^ 0xabcd));
    c.reply(1 | (255 << 1));
    c.check_replied();
    for (a9n::word i = 4; i < 259; ++i)
      CHECK(a9n::hal::get_message_register(c.client, i).unwrap() ==
            (i ^ 0xabcd));
  }
  { // Capability transfer completes before the direct reply continuation.
    conversation c;
    ipc_buffer source_buffer{}, destination_buffer{};
    capability_slot source{}, destination{}, destination_node_slot{};
    source.type = capability_type::FRAME;
    source.rights = capability_slot::ALL;
    source.data[0] = 0xbeef;
    test_node source_root(source), destination_node(destination);
    destination_node_slot.type = capability_type::NODE;
    destination_node_slot.component = &destination_node;
    test_node destination_root(destination_node_slot);
    c.server.root_slot.component = &source_root;
    c.client.root_slot.component = &destination_root;
    c.server.buffer = &source_buffer;
    c.client.buffer = &destination_buffer;
    c.server.buffer_frame.type = c.client.buffer_frame.type = capability_type::FRAME;
    c.reply(1 | (1 << 9));
    c.check_replied();
    CHECK(cpu_local_variables[0].current_process == &c.client);
    CHECK(!c.client.is_in_ready_queue);
    CHECK(source.type == capability_type::NONE);
    CHECK(destination.type == capability_type::FRAME);
    CHECK(destination.data[0] == 0xbeef);
  }
  { // Nested call/reply leaves the intermediate server's own reply intact.
    conversation c;
    ipc_port nested;
    auto nested_server = ready(PRIORITY_MAX - 1);
    CHECK(mark_scheduled(nested_server, c.server));
    cpu_local_variables[0].current_process = &nested_server;
    nested_server.registers[1] = 2;
    nested_server.registers[2] = 1;
    CHECK(nested.execute(nested_server, c.slot));
    CHECK(cpu_local_variables[0].current_process == &c.server);
    c.server.registers[1] = 3;
    c.server.registers[2] = 1;
    CHECK(nested.execute(c.server, c.slot));
    CHECK(cpu_local_variables[0].current_process == &nested_server);
    nested_server.registers[1] = 5;
    nested_server.registers[2] = 1;
    CHECK(nested.execute(nested_server, c.slot));
    CHECK(cpu_local_variables[0].current_process == &c.server);
    CHECK(c.server.destination_reply_target == &c.client);
    c.reply();
    c.check_replied();
    CHECK(cpu_local_variables[0].current_process == &c.client);
  }
  { // A bound port with no pending notification does not prevent the fastpath.
    conversation c;
    notification_port notification;
    c.server.binded_notification_port.type = capability_type::NOTIFICATION_PORT;
    c.server.binded_notification_port.component = &notification;
    CHECK(notification.bind_process(c.server));
    c.reply();
    c.check_replied();
    CHECK(cpu_local_variables[0].current_process == &c.client);
  }
  { // A pending bound notification keeps the server running after replying.
    conversation c;
    notification_port notification;
    capability_slot ns{};
    ns.rights = capability_slot::ALL;
    ns.data[0] = 0x80;
    c.server.binded_notification_port.type = capability_type::NOTIFICATION_PORT;
    c.server.binded_notification_port.component = &notification;
    CHECK(notification.bind_process(c.server));
    CHECK(notification.operation_notify(c.server, ns));
    c.reply();
    c.check_replied();
    CHECK(cpu_local_variables[0].current_process == &c.server);
    CHECK(c.server.status == process_status::READY);
    CHECK(c.client.is_in_ready_queue);
    CHECK(c.server.registers[2] == 0x4000);
    CHECK(c.server.registers[3] == 0x80);
    CHECK(!notification.has_pending_notification());
  }
  { // Nonblocking receive must not block the server.
    conversation c;
    c.reply(0);
    c.check_replied();
    CHECK(cpu_local_variables[0].current_process == &c.server);
    CHECK(c.client.is_in_ready_queue);
    CHECK(c.server.current_ipc_port == nullptr);
  }
  { // A second receiving server retains its position in the IPC wait queue.
    conversation c;
    auto receiver = ready(2);
    auto runnable = ready(1);
    CHECK(mark_scheduled(c.server, runnable));
    CHECK(c.invoke(receiver, 2));
    cpu_local_variables[0].current_process = &c.server;
    c.reply();
    c.check_replied();
    CHECK(receiver.preview_ipc_queue == nullptr);
    CHECK(receiver.next_ipc_queue == &c.server);
    CHECK(c.server.preview_ipc_queue == &receiver);
    c.call();
    CHECK(receiver.destination_reply_target == &c.client);
    CHECK(receiver.current_ipc_port == nullptr);
    CHECK(receiver.preview_ipc_queue == nullptr);
    CHECK(receiver.next_ipc_queue == nullptr);
    CHECK(c.server.current_ipc_port == &c.port);
    CHECK(c.server.preview_ipc_queue == nullptr);
    CHECK(c.server.next_ipc_queue == nullptr);
  }
  for (unsigned removed = 0; removed < 3; ++removed) {
    // Removing the head, middle or tail must preserve the next pop's invariants.
    conversation c;
    auto first = ready(PRIORITY_MAX - 1), second = ready(PRIORITY_MAX - 1);
    CHECK(mark_scheduled(c.server, first));
    CHECK(mark_scheduled(c.server, second));
    c.reply();
    CHECK(cpu_local_variables[0].current_process == &first);
    CHECK(c.invoke(first, 2));
    CHECK(cpu_local_variables[0].current_process == &second);
    CHECK(c.invoke(second, 2));
    CHECK(cpu_local_variables[0].current_process == &c.client);
    process *receivers[] = {&c.server, &first, &second};
    notification_port notification;
    capability_slot notification_slot{};
    notification_slot.rights = capability_slot::ALL;
    notification_slot.data[0] = 1;
    auto &notified = *receivers[removed];
    notified.priority = c.client.priority - 1;
    notified.binded_notification_port.type = capability_type::NOTIFICATION_PORT;
    notified.binded_notification_port.component = &notification;
    CHECK(notification.bind_process(notified));
    CHECK(notification.operation_notify(c.client, notification_slot));
    CHECK(cpu_local_variables[0].current_process == &c.client);
    CHECK(receivers[removed]->current_ipc_port == nullptr);
    CHECK(receivers[removed]->preview_ipc_queue == nullptr);
    CHECK(receivers[removed]->next_ipc_queue == nullptr);
    auto *head = receivers[removed == 0 ? 1 : 0];
    auto *tail = receivers[removed == 2 ? 1 : 2];
    CHECK(head->preview_ipc_queue == nullptr);
    CHECK(head->next_ipc_queue == tail);
    CHECK(tail->preview_ipc_queue == head);
    CHECK(tail->next_ipc_queue == nullptr);
    c.call();
    CHECK(cpu_local_variables[0].current_process == head);
    CHECK(head->current_ipc_port == nullptr);
    CHECK(head->preview_ipc_queue == nullptr);
    CHECK(head->next_ipc_queue == nullptr);
    CHECK(tail->current_ipc_port == &c.port);
    CHECK(tail->preview_ipc_queue == nullptr);
    CHECK(tail->next_ipc_queue == nullptr);
  }
  { // Reply-Receive with no prior caller remains a normal blocking receive.
    conversation c;
    c.reply();
    ipc_port fresh;
    c.client.registers[1] = 5;
    c.client.registers[2] = 1;
    auto runnable = ready(2);
    CHECK(mark_scheduled(c.client, runnable));
    CHECK(fresh.execute(c.client, c.slot));
    CHECK(c.client.status == process_status::BLOCKED_RECEIVE);
    CHECK(cpu_local_variables[0].current_process == &runnable);
  }
  { // Existing READY_TO_SEND path: accept queued caller without switching.
    conversation c;
    auto sender = ready(1);
    CHECK(c.invoke(sender, 3));
    cpu_local_variables[0].current_process = &c.server;
    const auto before = switches;
    c.reply();
    CHECK(switches == before);
    CHECK(c.client.is_in_ready_queue);
    CHECK(c.server.destination_reply_target == &sender);
    CHECK(sender.status == process_status::BLOCKED_REPLY);
  }
  { // Fault replies retain their specialized handling.
    conversation c;
    c.client.fault_reason = fault_type::MEMORY;
    c.client.status = process_status::BLOCKED_FAULT;
    c.reply();
    c.check_replied();
    CHECK(c.client.fault_reason == fault_type::NONE);
  }
  { // Transfer failure must not commit/block either participant.
    conversation c;
    const auto before = switches;
    CHECK(!c.invoke(c.server, 5, 1 | (7 << 1))); // no IPC buffers
    CHECK(switches == before);
    CHECK(c.server.status == process_status::READY);
    CHECK(c.client.status == process_status::BLOCKED_REPLY);
    CHECK(c.server.destination_reply_target == &c.client);
    CHECK(!c.invoke(c.server, 5,
                    1 | (1 << 9))); // capability transfer: general path
    CHECK(c.client.status == process_status::BLOCKED_REPLY);
  }
  if constexpr (SMP_ENABLED) {
    conversation c;
    auto other = ready(2);
    CHECK(mark_scheduled(c.server, other));
    c.client.core_affinity = 1;
    c.reply();
    c.check_replied();
    CHECK(ipis == 1);
    CHECK(c.client.is_in_ready_queue);
    CHECK(cpu_local_variables[1].pending_reschedule_target == &c.client);
    CHECK(cpu_local_variables[0].current_process == &other);
  }
}

static void payload_tests() {
  constexpr a9n::word payload_start = 4;
  constexpr a9n::word register_count = a9n::hal::MESSAGE_REGISTER_COUNT - payload_start;
  constexpr a9n::word untouched = 0xfedcba9876543210;
  for (unsigned operation = 3; operation <= 5; ++operation) {
    for (unsigned buffers = 0; buffers < 4; ++buffers) {
      for (a9n::word length = 0; length <= 255; ++length) {
        conversation c;
        if (operation == 3)
          c.reply(); // Park the server before testing Call.
        auto &source = operation == 3 ? c.client : c.server;
        auto &destination = operation == 3 ? c.server : c.client;
        ipc_buffer source_buffer{}, destination_buffer{};
        source.buffer = buffers & 1 ? &source_buffer : nullptr;
        destination.buffer = buffers & 2 ? &destination_buffer : nullptr;
        for (a9n::word i = 0; i <= 255; ++i) {
          const auto value = 0x10000 + length * 256 + i;
          source_buffer.messages[payload_start + i] = value;
          destination_buffer.messages[payload_start + i] = untouched;
          if (i < register_count) {
            source.registers[payload_start + i] = value;
            destination.registers[payload_start + i] = untouched;
          }
        }
        const bool succeeds = length <= register_count || buffers == 3;
        const auto before = switches;
        CHECK(bool(c.invoke(source, operation, 1 | (length << 1))) == succeeds);
        if (!succeeds)
          CHECK(switches == before);
        const auto copied = succeeds ? length : register_count;
        for (a9n::word i = 0; i <= 255; ++i) {
          const auto actual = i < register_count
              ? static_cast<a9n::word>(destination.registers[payload_start + i])
              : destination_buffer.messages[payload_start + i];
          CHECK(actual == (i < copied ? 0x10000 + length * 256 + i : untouched));
        }
      }
    }
  }
}

int main() {
  scheduler_tests();
  ipc_tests();
  payload_tests();
  printf("Priority range: 0..%lld\n", static_cast<long long>(PRIORITY_MAX - 1));
  puts("PASS scheduler FIFO (20000 steps), IPC round trips (1000), "
       "fallback cases, and Call/Reply/Reply-Receive payloads (0..255 words)");
}
