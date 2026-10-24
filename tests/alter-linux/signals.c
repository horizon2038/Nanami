/* Freestanding x86_64 Linux ABI regression. No libc or FPU instructions required. */
typedef unsigned long word;
extern long linux_call(long, word, word, word, word, word, word);
extern void signal_restorer(void);
extern void invalid_instruction(void);
struct action { word handler, flags, restorer, mask; };
struct stack { word sp, flags, size; };
struct info { unsigned int signal, error; int code; unsigned int pad; unsigned char data[112]; };
struct context { word flags, link; struct stack stack; word gregs[32]; word mask; };
static volatile int received, nested, bad_handler, alarm_count;
static volatile word handler_stack;
static unsigned char alternate[16384];
static word self;

static long call(long number, word a, word b, word c, word d) {
    return linux_call(number, a, b, c, d, 0, 0);
}
static void handler(int signal, struct info *info, struct context *context) {
    word mask = 0;
    handler_stack = (word)&mask;
    if (info->signal != (unsigned int)signal || context == 0) { bad_handler = 1; }
    if (signal == 4) {
        context->gregs[16] += 2; /* Skip the deliberate UD2 instruction. */
        return;
    }
    if (signal == 14) { alarm_count++; return; }
    if (call(14, 2, 0, (word)&mask, 8) != 0 || (mask & (1ul << (signal - 1))) == 0) {
        bad_handler = 2;
    }
    received++;
    if (signal == 10 && nested != 0) {
        nested = 0;
        call(62, self, 12, 0, 0);
    }
}
static int install(int signal, word flags) {
    struct action action = { (word)handler, flags | 4 | 0x04000000, (word)signal_restorer, 0 };
    return call(13, signal, (word)&action, 0, 8) == 0;
}
static int wait_child(word child, unsigned int expected, word options) {
    unsigned int status = 0;
    return call(61, child, (word)&status, options, 0) == (long)child && status == expected;
}
static void finish(int status) {
    call(60, status, 0, 0, 0);
}
int test_main(void) {
    self = call(39, 0, 0, 0, 0);
    if (!install(10, 0) || !install(12, 0)) { return 10; }
    if (call(62, self, 10, 0, 0) != 0 || received != 1 || bad_handler != 0) { return 11; }
    word mask = 1ul << 9, pending = 0;
    if (call(14, 0, (word)&mask, 0, 8) != 0) { return 12; }
    call(62, self, 10, 0, 0);
    call(62, self, 10, 0, 0);
    if (call(127, (word)&pending, 8, 0, 0) != 0 || pending != mask || received != 1) { return 13; }
    if (call(14, 1, (word)&mask, 0, 8) != 0 || received != 2) { return 14; }
    nested = 1;
    call(62, self, 10, 0, 0);
    if (received != 4 || bad_handler != 0) { return 15; }
    struct action without_restorer = { (word)handler, 4, 0, 0 };
    if (call(13, 10, (word)&without_restorer, 0, 8) != 0) { return 50; }
    call(62, self, 10, 0, 0);
    if (received != 5) { return 51; }

    struct stack stack = { (word)alternate, 0, sizeof(alternate) }, old = { 0, 0, 0 };
    if (call(131, (word)&stack, 0, 0, 0) != 0 || !install(10, 0x08000000)) { return 16; }
    call(62, self, 10, 0, 0);
    if (handler_stack < stack.sp || handler_stack >= stack.sp + stack.size) { return 17; }
    if (call(131, 0, (word)&old, 0, 0) != 0 || old.sp != stack.sp || old.flags != 0) { return 18; }
    struct action invalid = { (word)handler, 0, 0, 0 };
    if (call(13, 9, (word)&invalid, 0, 8) != -22) { return 19; }
    mask = ~0ul;
    call(14, 2, (word)&mask, 0, 8);
    call(14, 2, 0, (word)&pending, 8);
    if ((pending & ((1ul << 8) | (1ul << 18))) != 0) { return 20; }
    mask = 0;
    call(14, 2, (word)&mask, 0, 8);

    /* Realtime signals retain siginfo and FIFO order, unlike standard signals. */
    mask = 1ul << 33;
    call(14, 0, (word)&mask, 0, 8);
    struct info info = { 34, 0, -1, 0, { 0 } };
    info.data[8] = 41;
    if (call(129, self, 34, (word)&info, 0) != 0) { return 21; }
    info.data[8] = 42;
    if (call(129, self, 34, (word)&info, 0) != 0) { return 22; }
    word zero_time[2] = { 0, 0 };
    if (call(128, (word)&mask, (word)&info, (word)zero_time, 8) != 34 || info.data[8] != 41) { return 23; }
    if (call(128, (word)&mask, (word)&info, (word)zero_time, 8) != 34 || info.data[8] != 42) { return 24; }
    if (call(128, (word)&mask, 0, (word)zero_time, 8) != -11) { return 25; }
    call(14, 1, (word)&mask, 0, 8);

    if (!install(4, 0)) { return 26; }
    invalid_instruction();

    word child = call(57, 0, 0, 0, 0);
    if ((long)child < 0) { return 27; }
    if (child == 0) { call(62, call(39, 0, 0, 0, 0), 19, 0, 0); finish(7); }
    if (!wait_child(child, (19u << 8) | 0x7f, 2)) { return 28; }
    if (call(62, child, 18, 0, 0) != 0 || !wait_child(child, 7u << 8, 0)) { return 29; }
    child = call(57, 0, 0, 0, 0);
    if (child == 0) { call(62, call(39, 0, 0, 0, 0), 15, 0, 0); finish(99); }
    if (!wait_child(child, 15, 0)) { return 30; }
    child = call(57, 0, 0, 0, 0);
    if (child == 0) { call(15, 0, 0, 0, 0); finish(99); }
    if (!wait_child(child, 11, 0)) { return 52; }

    /* SIGALRM interrupts nanosleep, preserving its remaining interval. */
    if (!install(14, 0)) { return 31; }
    word itimer[4] = { 0, 0, 0, 20000 };
    if (call(38, 0, (word)itimer, 0, 0) != 0) { return 32; }
    word sleep[2] = { 2, 0 }, remain[2] = { 0, 0 };
    if (call(35, (word)sleep, (word)remain, 0, 0) != -4 || alarm_count != 1 || remain[0] == 0) { return 33; }
    if (call(38, 1, (word)itimer, 0, 0) != -95) { return 34; }

    /* A signal delivered to a parked pipe reader either restarts or returns EINTR. */
    for (int restart = 0; restart != 2; restart++) {
        int pipe[2] = { -1, -1 };
        if (call(22, (word)pipe, 0, 0, 0) != 0 || !install(10, restart ? 0x10000000 : 0)) { return 35; }
        child = call(57, 0, 0, 0, 0);
        if (child == 0) {
            word delay[2] = { 0, 20000000 };
            call(3, pipe[0], 0, 0, 0);
            call(35, (word)delay, 0, 0, 0);
            call(62, self, 10, 0, 0);
            call(1, pipe[1], (word)"x", 1, 0);
            finish(0);
        }
        call(3, pipe[1], 0, 0, 0);
        char byte = 0;
        long result = call(0, pipe[0], (word)&byte, 1, 0);
        if (restart && (result != 1 || byte != 'x')) { return 36; }
        if (!restart && result != -4) { return 37; }
        /* Leave the read end open until the child has completed its write. */
        if (!wait_child(child, 0, 0)) { return 38; }
        call(3, pipe[0], 0, 0, 0);
    }
    mask = 1ul << 13;
    call(14, 0, (word)&mask, 0, 8);
    call(38, 0, (word)itimer, 0, 0);
    word temporary_mask = 0;
    if (call(130, (word)&temporary_mask, 8, 0, 0) != -4 || alarm_count != 2) { return 41; }
    pending = 0;
    call(14, 2, 0, (word)&pending, 8);
    if ((pending & mask) == 0) { return 42; }
    call(14, 1, (word)&mask, 0, 8);
    /* A timed sigwait consumes SIGALRM without entering its installed handler. */
    call(14, 0, (word)&mask, 0, 8);
    call(38, 0, (word)itimer, 0, 0);
    if (call(128, (word)&mask, (word)&info, (word)sleep, 8) != 14 || alarm_count != 2) { return 43; }
    call(14, 1, (word)&mask, 0, 8);
    word short_time[2] = { 0, 1000000 };
    if (call(128, (word)&mask, 0, (word)short_time, 8) != -11) { return 44; }

    int paused_pipe[2] = { -1, -1 };
    if (call(22, (word)paused_pipe, 0, 0, 0) != 0) { return 45; }
    child = call(57, 0, 0, 0, 0);
    if (child == 0) {
        char byte = 0;
        call(3, paused_pipe[1], 0, 0, 0);
        if (call(0, paused_pipe[0], (word)&byte, 1, 0) != 1 || byte != 'y') { finish(90); }
        finish(0);
    }
    call(3, paused_pipe[0], 0, 0, 0);
    word delay[2] = { 0, 20000000 };
    call(35, (word)delay, 0, 0, 0);
    call(62, child, 19, 0, 0);
    if (!wait_child(child, (19u << 8) | 0x7f, 2)) { return 46; }
    call(62, child, 18, 0, 0);
    if (!wait_child(child, 0xffff, 8)) { return 47; }
    call(1, paused_pipe[1], (word)"y", 1, 0);
    if (!wait_child(child, 0, 0)) { return 48; }
    call(3, paused_pipe[1], 0, 0, 0);

    child = call(57, 0, 0, 0, 0);
    if (child == 0) { finish(3); }
    call(35, (word)delay, 0, 0, 0);
    word second_child = call(57, 0, 0, 0, 0);
    if (second_child == 0) { finish(4); }
    if (!wait_child(child, 3u << 8, 0)) { return 53; }
    if (!wait_child(second_child, 4u << 8, 0)) { return 54; }

    struct action ignore_child = { 1, 0, 0, 0 };
    call(13, 17, (word)&ignore_child, 0, 8);
    child = call(57, 0, 0, 0, 0);
    if (child == 0) { finish(0); }
    unsigned int status = 0;
    if (call(61, child, (word)&status, 0, 0) != -10) { return 49; }
    if (bad_handler != 0) { return 39; }
    long fd = call(2, (word)"/work/signal-abi", 1 | 64 | 512, 0644, 0);
    if (fd < 0 || call(1, fd, (word)"SIGNAL-ABI-PASS\n", 16, 0) != 16) { return 40; }
    call(3, fd, 0, 0, 0);
    return 0;
}
