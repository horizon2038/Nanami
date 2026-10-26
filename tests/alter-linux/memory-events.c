/* Integer-only Linux ABI regression: real mappings, descriptor events and futexes. */
typedef unsigned long word;
extern long linux_call(long, word, word, word, word, word, word);
extern void signal_restorer(void);
static _Atomic unsigned int handled;
static void handle_signal(int signal) { handled += (signal == 10); }
static long call(long n, word a, word b, word c, word d) {
    return linux_call(n, a, b, c, d, 0, 0);
}
static int child_status(word child, unsigned int expected) {
    unsigned int status = 0;
    return call(61, child, (word)&status, 0, 0) == (long)child && status == expected;
}
static void child_exit(int code) { call(60, code, 0, 0, 0); }
int test_main(void) {
    long fd = call(319, (word)"shared-buffer", 2, 0, 0);
    if (fd < 0 || call(77, fd, 8192, 0, 0) != 0) { return 10; }
    long mapped = linux_call(9, 0, 8192, 3, 1, fd, 0);
    if (mapped < 0) { return 11; }
    volatile unsigned int *buffer = (volatile unsigned int *)mapped;
    buffer[0] = 0x12345678;
    int sockets[2];
    unsigned int control[6] = { 20, 0, 1, 1, (unsigned int)fd, 0 };
    unsigned int received_control[6] = { 0 };
    char payload = 'x', received_payload = 0;
    word send_iov[2] = { (word)&payload, 1 }, recv_iov[2] = { (word)&received_payload, 1 };
    word send_header[7] = { 0, 0, (word)send_iov, 1, (word)control, sizeof(control), 0 };
    word recv_header[7] = { 0, 0, (word)recv_iov, 1, (word)received_control, sizeof(received_control), 0 };
    if (call(53, 1, 2, 0, (word)sockets) != 0 || call(46, sockets[0], (word)send_header, 0, 0) != 1 || call(47, sockets[1], (word)recv_header, 0, 0) != 1) { return 37; }
    long passed = linux_call(9, 0, 4096, 3, 1, received_control[4], 0);
    if (passed < 0 || *(volatile unsigned int *)passed != buffer[0]) { return 38; }
    *(volatile unsigned int *)passed = 0x10203040;
    if (buffer[0] != 0x10203040) { return 39; }
    call(11, passed, 4096, 0, 0);
    call(3, received_control[4], 0, 0, 0);
    call(3, sockets[0], 0, 0, 0);
    call(3, sockets[1], 0, 0, 0);
    buffer[0] = 0x12345678;
    if (call(10, mapped, 4096, 0, 0) != 0 || call(10, mapped, 4096, 3, 0) != 0 || buffer[0] != 0x12345678) { return 12; }
    long child = call(57, 0, 0, 0, 0);
    if (child == 0) {
        buffer[0] = 0x87654321;
        child_exit(0);
    }
    if (child < 0 || !child_status(child, 0) || buffer[0] != 0x87654321) { return 13; }
    child = call(57, 0, 0, 0, 0);
    if (child == 0) {
        if (call(10, mapped, 4096, 1, 0) != 0) { child_exit(90); }
        buffer[0] = 1; /* Must produce SIGSEGV, not a writable alias or a kernel panic. */
        child_exit(91);
    }
    if (child < 0 || !child_status(child, 11)) { return 14; }
    if (call(77, fd, 17, 0, 0) != 0) { return 15; }
    child = call(57, 0, 0, 0, 0);
    if (child == 0) {
        buffer[1024] = 1; /* The first page beyond EOF must produce SIGBUS. */
        child_exit(92);
    }
    if (child < 0 || !child_status(child, 7)) { return 16; }
    if (call(77, fd, 8192, 0, 0) != 0 || buffer[1024] != 0) { return 17; }
    buffer[0] = 1;
    child = call(57, 0, 0, 0, 0);
    if (child == 0) {
        word delay[2] = { 0, 10000000 };
        call(35, (word)delay, 0, 0, 0);
        buffer[0] = 2;
        call(202, mapped, 1, 1, 0);
        child_exit(0);
    }
    word timeout[2] = { 2, 0 };
    long waited = call(202, mapped, 0, 1, (word)timeout);
    if ((waited != 0 && waited != -11) || child < 0 || !child_status(child, 0) || buffer[0] != 2) { return 18; }
    if (call(11, mapped, 8192, 0, 0) != 0 || call(72, fd, 1033, 2 | 4 | 8, 0) != 0) { return 19; }
    if (call(77, fd, 0, 0, 0) != -1) { return 20; }
    call(3, fd, 0, 0, 0);

    long private_memory = linux_call(9, 0, 4096, 3, 0x22, (word)-1, 0);
    if (private_memory < 0) { return 30; }
    *(volatile word *)private_memory = 0x13579bdf;
    long counter = call(290, 1, 0x800, 0, 0);
    if (counter < 0 || call(10, private_memory, 4096, 0, 0) != 0) { return 31; }
    if (call(0, counter, private_memory, 8, 0) != -14 || call(1, 1, private_memory, 1, 0) != -14) { return 32; }
    word value = 0;
    if (call(0, counter, (word)&value, 8, 0) != 8 || value != 1) { return 33; }
    child = call(57, 0, 0, 0, 0);
    if (child == 0) {
        if (call(10, private_memory, 4096, 1, 0) != 0 || *(volatile word *)private_memory != 0x13579bdf) { child_exit(94); }
        child_exit(0);
    }
    if (child < 0 || !child_status(child, 0)) { return 34; }
    long moved = linux_call(25, private_memory, 4096, 8192, 3, 0x200000000ul, 0);
    if (moved != 0x200000000ul || call(10, moved, 4096, 1, 0) != 0 || *(volatile word *)moved != 0x13579bdf) { return 35; }
    if (call(10, moved + 4096, 4096, 3, 0) != 0 || *(volatile word *)(moved + 4096) != 0) { return 36; }
    call(11, moved, 8192, 0, 0);
    call(3, counter, 0, 0, 0);

    long timer = call(283, 1, 0x800, 0, 0);
    long epoll = call(291, 0, 0, 0, 0);
    word setting[4] = { 0, 10000000, 0, 20000000 };
    unsigned char event[12] = { 1, 0, 0, 0, 0x88, 0x77, 0x66, 0x55, 0, 0, 0, 0 };
    if (timer < 0 || epoll < 0 || call(233, epoll, 1, timer, (word)&event) != 0 || call(286, timer, 0, (word)setting, 0) != 0) { return 21; }
    unsigned char output[12] = { 0 };
    if (call(232, epoll, (word)output, 1, 1000) != 1) { return 22; }
    for (int index = 4; index < 12; index++) {
        if (output[index] != event[index]) { return 22; }
    }
    word count = 0;
    if (call(0, timer, (word)&count, 8, 0) != 8 || count == 0) { return 23; }
    call(3, timer, 0, 0, 0);
    call(3, epoll, 0, 0, 0);
    word mask = 1ul << 9;
    if (call(14, 0, (word)&mask, 0, 8) != 0) { return 24; }
    long signals = call(289, (word)-1, (word)&mask, 8, 0x800);
    long self = call(39, 0, 0, 0, 0);
    if (signals < 0 || call(62, self, 10, 0, 0) != 0) { return 25; }
    unsigned int info[32];
    if (call(0, signals, (word)info, sizeof(info), 0) != sizeof(info) || info[0] != 10 || info[3] != (unsigned int)self) { return 26; }
    if (call(0, signals, (word)info, sizeof(info), 0) != -11) { return 27; }
    call(3, signals, 0, 0, 0);
    word action[4] = { (word)handle_signal, 0x04000000, (word)signal_restorer, 0 };
    word empty_mask = 0, waiting_time[2] = { 2, 0 }, restored_mask = 0;
    if (call(13, 10, (word)action, 0, 8) != 0 || call(62, self, 10, 0, 0) != 0) { return 40; }
    if (linux_call(271, 0, 0, (word)waiting_time, (word)&empty_mask, 8, 0) != -4 || handled != 1) { return 41; }
    if (call(14, 0, 0, (word)&restored_mask, 8) != 0 || restored_mask != mask) { return 42; }
    word mask_argument[2] = { (word)&empty_mask, 8 };
    if (call(62, self, 10, 0, 0) != 0 || linux_call(270, 0, 0, 0, 0, (word)waiting_time, (word)mask_argument) != -4 || handled != 2) { return 43; }
    epoll = call(291, 0, 0, 0, 0);
    if (epoll < 0 || call(62, self, 10, 0, 0) != 0 || linux_call(281, epoll, (word)output, 1, 1000, (word)&empty_mask, 8) != -4 || handled != 3) { return 44; }
    call(3, epoll, 0, 0, 0);
    call(14, 1, (word)&mask, 0, 8);
    static const char pass[] = "MEMORY-EVENTS-PASS\n";
    call(1, 1, (word)pass, sizeof(pass) - 1, 0);
    return 0;
}
