/* Integer-only regressions for Neovim's umask, flock and io_uring probes. */
typedef unsigned long word;
typedef unsigned int u32;
typedef unsigned long long u64;
extern long linux_call(long, word, word, word, word, word, word);
extern void signal_restorer(void);
void *memset(void *pointer, int value, word bytes) {
    volatile unsigned char *target = pointer;
    for (word i = 0; i < bytes; ++i) { target[i] = (unsigned char)value; }
    return pointer;
}
static long call(long n, word a, word b, word c, word d) {
    return linux_call(n, a, b, c, d, 0, 0);
}
static int wait_child(long pid) {
    u32 status = 0;
    return pid > 0 && call(61, pid, (word)&status, 0, 0) == pid && status == 0;
}
static void child_exit(int value) { call(60, value, 0, 0, 0); }
static void delay(void) {
    word time[2] = { 0, 50000000 };
    call(35, (word)time, 0, 0, 0);
}
static u32 mode(long fd) {
    u64 stat[18] = { 0 };
    if (call(5, fd, (word)stat, 0, 0) != 0) { return ~0u; }
    return *(u32 *)((char *)stat + 24) & 07777;
}
static void signal_handler(int signal) { (void)signal; }
static int masks_and_locks(void) {
    call(87, (word)"/tmp/nvim-lock", 0, 0, 0);
    call(87, (word)"/tmp/nvim-zero", 0, 0, 0);
    call(84, (word)"/tmp/nvim-mask", 0, 0, 0);
    if (call(95, 077, 0, 0, 0) != 022) { return 10; }
    long a = call(2, (word)"/tmp/nvim-lock", 0102, 0666, 0);
    if (a < 0 || mode(a) != 0600) { return 11; }
    long zero = call(2, (word)"/tmp/nvim-zero", 0102, 0, 0);
    if (zero < 0 || mode(zero) != 0) { return 12; }
    call(3, zero, 0, 0, 0);
    if (call(83, (word)"/tmp/nvim-mask", 0777, 0, 0) != 0) { return 13; }
    long directory = call(2, (word)"/tmp/nvim-mask", 0200000, 0, 0);
    if (directory < 0 || mode(directory) != 0700) { return 14; }
    call(3, directory, 0, 0, 0);
    long child = call(57, 0, 0, 0, 0);
    if (child == 0) { child_exit(call(95, 002, 0, 0, 0) == 077 ? 0 : 90); }
    if (!wait_child(child) || call(95, 077, 0, 0, 0) != 077) { return 15; }
    long b = call(2, (word)"/tmp/nvim-lock", 2, 0, 0);
    long alias = call(32, a, 0, 0, 0);
    if (b < 0 || alias < 0 || call(73, a, 2, 0, 0) != 0 || call(73, b, 6, 0, 0) != -11) { return 20; }
    if (call(73, alias, 8, 0, 0) != 0 || call(73, a, 1, 0, 0) != 0 || call(73, b, 5, 0, 0) != 0) { return 21; }
    if (call(73, a, 6, 0, 0) != -11 || call(73, b, 6, 0, 0) != 0) { return 22; }
    if (call(73, b, 8, 0, 0) != 0 || call(73, a, 2, 0, 0) != 0) { return 23; }
    call(3, a, 0, 0, 0);
    if (call(73, b, 6, 0, 0) != -11) { return 24; }
    call(3, alias, 0, 0, 0);
    if (call(73, b, 6, 0, 0) != 0 || call(73, b, 8, 0, 0) != 0) { return 25; }
    a = call(2, (word)"/tmp/nvim-lock", 2, 0, 0);
    int pipe[2];
    if (call(22, (word)pipe, 0, 0, 0) != 0 || call(73, a, 2, 0, 0) != 0) { return 26; }
    child = call(57, 0, 0, 0, 0);
    if (child == 0) {
        char byte = 'r';
        call(3, a, 0, 0, 0);
        call(1, pipe[1], (word)&byte, 1, 0);
        child_exit(call(73, b, 2, 0, 0) == 0 ? 0 : 91);
    }
    char byte;
    if (child < 0 || call(0, pipe[0], (word)&byte, 1, 0) != 1) { return 27; }
    delay();
    if (call(73, a, 8, 0, 0) != 0 || !wait_child(child)) { return 28; }
    /* Child and parent share b's description; its successful lock survives exit. */
    if (call(73, a, 6, 0, 0) != -11 || call(73, b, 8, 0, 0) != 0) { return 29; }
    word action[4] = { (word)signal_handler, 0x04000000, (word)signal_restorer, 0 };
    if (linux_call(13, 10, (word)action, 0, 8, 0, 0) != 0 || call(73, a, 2, 0, 0) != 0) { return 30; }
    child = call(57, 0, 0, 0, 0);
    if (child == 0) {
        call(3, a, 0, 0, 0);
        byte = 's';
        call(1, pipe[1], (word)&byte, 1, 0);
        child_exit(call(73, b, 2, 0, 0) == -4 ? 0 : 92);
    }
    if (child < 0 || call(0, pipe[0], (word)&byte, 1, 0) != 1) { return 31; }
    delay();
    if (call(62, child, 10, 0, 0) != 0 || !wait_child(child)) { return 32; }
    action[1] |= 0x10000000; /* SA_RESTART replays the interrupted flock. */
    if (linux_call(13, 10, (word)action, 0, 8, 0, 0) != 0) { return 34; }
    child = call(57, 0, 0, 0, 0);
    if (child == 0) {
        call(3, a, 0, 0, 0);
        byte = 't';
        call(1, pipe[1], (word)&byte, 1, 0);
        child_exit(call(73, b, 2, 0, 0) == 0 ? 0 : 93);
    }
    if (child < 0 || call(0, pipe[0], (word)&byte, 1, 0) != 1) { return 35; }
    delay();
    if (call(62, child, 10, 0, 0) != 0) { return 36; }
    delay();
    if (call(73, a, 8, 0, 0) != 0 || !wait_child(child)) { return 37; }
    call(3, a, 0, 0, 0);
    call(3, b, 0, 0, 0);
    call(3, pipe[0], 0, 0, 0);
    call(3, pipe[1], 0, 0, 0);
    if (call(73, -1, 2, 0, 0) != -9) { return 33; }
    call(95, 022, 0, 0, 0);
    return 0;
}
struct sqe {
    unsigned char opcode, flags;
    unsigned short priority;
    int fd;
    u64 off, addr;
    u32 len, op_flags;
    u64 data;
    u64 reserved[3];
};
_Static_assert(sizeof(struct sqe) == 64, "Linux SQE ABI");
struct ring {
    long fd;
    u32 params[30];
    char *memory;
    struct sqe *sqes;
    u32 submitted, consumed;
};
static struct ring ring;
static void put(u32 offset, u32 value) {
    *((_Atomic u32 *)(ring.memory + offset)) = value;
}
static u32 get(u32 offset) {
    return *((_Atomic u32 *)(ring.memory + offset));
}
static void submit(unsigned char opcode, int fd, word off, word addr, u32 len, u32 flags, u64 data) {
    u32 index = ring.submitted & (ring.params[0] - 1);
    volatile char *bytes = (volatile char *)&ring.sqes[index];
    for (u32 i = 0; i < 64; ++i) { bytes[i] = 0; }
    ring.sqes[index].opcode = opcode;
    ring.sqes[index].fd = fd;
    ring.sqes[index].off = off;
    ring.sqes[index].addr = addr;
    ring.sqes[index].len = len;
    ring.sqes[index].op_flags = flags;
    ring.sqes[index].data = data;
    if (ring.params[16] != 0) { *(u32 *)(ring.memory + ring.params[16] + index * 4) = index; }
    put(ring.params[11], ++ring.submitted);
}
static int completion(u64 data, int result) {
    if (get(ring.params[21]) == ring.consumed) { return 0; }
    char *cqe = ring.memory + ring.params[25] + (ring.consumed & (ring.params[1] - 1)) * 16;
    if (*(u64 *)cqe != data || *(int *)(cqe + 8) != result || *(u32 *)(cqe + 12) != 0) { return 0; }
    put(ring.params[20], ++ring.consumed);
    return 1;
}
static int rings(void) {
    u32 bad[30] = { 0 };
    bad[2] = 2; /* SQPOLL requires a worker, so it must not be falsely accepted. */
    if (call(425, 4, (word)bad, 0, 0) != -22) { return 40; }
    ring.fd = call(425, 4, (word)ring.params, 0, 0);
    if (ring.fd < 0 || ring.params[0] != 4 || ring.params[1] != 8 || ring.params[5] != 3) { return 41; }
    long mapped = linux_call(9, 0, 4096, 3, 1, ring.fd, 0);
    long sqes = linux_call(9, 0, 4096, 3, 1, ring.fd, 0x10000000);
    if (mapped < 0 || sqes < 0) { return 42; }
    ring.memory = (char *)mapped;
    ring.sqes = (struct sqe *)sqes;
    unsigned char probe[16 + 64 * 8] = { 0 };
    if (call(427, ring.fd, 8, (word)probe, 64) != 0 || probe[0] != 55 || probe[1] != 56 || probe[16 + 29 * 8 + 2] != 1) { return 43; }
    submit(0, 0, 0, 0, 0, 0, 123);
    if (call(426, ring.fd, 1, 1, 1) != 1 || !completion(123, 0)) { return 44; }
    long child = call(57, 0, 0, 0, 0);
    if (child == 0) {
        submit(0, 0, 0, 0, 0, 0, 124);
        child_exit(call(426, ring.fd, 1, 0, 0) == 1 ? 0 : 94);
    }
    if (!wait_child(child) || !completion(124, 0)) { return 55; }
    ++ring.submitted;
    child = call(57, 0, 0, 0, 0);
    if (child == 0) {
        submit(0, 0, 0, 0, 0, 0, 125);
        child_exit(call(426, ring.fd, 1, 2, 1) == 1 ? 0 : 95);
    }
    if (child < 0) { return 56; }
    while (get(ring.params[11]) == ring.submitted) { delay(); }
    delay();
    if (call(62, child, 10, 0, 0) != 0 || !wait_child(child) || !completion(125, 0)) { return 57; }
    ++ring.submitted;
    long file = call(319, (word)"ring-file", 0, 0, 0);
    char text[] = "ring", readback[5] = { 0 };
    word iov[2] = { (word)text, 4 }, read_iov[2] = { (word)readback, 4 };
    if (file < 0) { return 45; }
    submit(2, file, 0, (word)iov, 1, 0, 200);
    submit(1, file, 0, (word)read_iov, 1, 0, 201);
    submit(255, 0, 0, 0, 0, 0, 202);
    if (call(426, ring.fd, 3, 3, 1) != 3 || !completion(200, 4) || !completion(201, 4) || !completion(202, -95)) { return 46; }
    if (readback[0] != 'r' || readback[1] != 'i' || readback[2] != 'n' || readback[3] != 'g') { return 47; }
    /* Exercise NODROP when the application temporarily stops consuming CQEs. */
    for (u32 batch = 0; batch < 3; ++batch) {
        for (u32 i = 0; i < 4; ++i) { submit(0, 0, 0, 0, 0, 0, 300 + batch * 4 + i); }
        if (call(426, ring.fd, 4, 0, 0) != 4) { return 48; }
    }
    for (u32 i = 0; i < 8; ++i) { if (!completion(300 + i, 0)) { return 49; } }
    if (call(426, ring.fd, 0, 4, 1) != 0) { return 50; }
    for (u32 i = 8; i < 12; ++i) { if (!completion(300 + i, 0)) { return 51; } }
    long alias = call(32, ring.fd, 0, 0, 0);
    if (alias < 0 || call(3, ring.fd, 0, 0, 0) != 0) { return 52; }
    ring.fd = alias;
    submit(0, 0, 0, 0, 0, 0, 400);
    if (call(426, ring.fd, 1, 1, 1) != 1 || !completion(400, 0)) { return 53; }
    call(3, ring.fd, 0, 0, 0);
    if (get(ring.params[21]) != ring.consumed) { return 54; }
    call(11, mapped, 4096, 0, 0);
    call(11, sqes, 4096, 0, 0);
    call(3, file, 0, 0, 0);
    return 0;
}
int test_main(void) {
    int result = masks_and_locks();
    if (result != 0) { return result; }
    result = rings();
    if (result != 0) { return result; }
    call(1, 1, (word)"nvim-syscalls-ok\n", 17, 0);
    return 0;
}
