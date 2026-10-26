/* No libc/FPU: real CLONE_VM, shared FD/FS tables, TLS and clear-child-tid. */
typedef unsigned long word;
extern long linux_call(long, word, word, word, word, word, word);
extern long thread_start(word, word, word, word, word, int (*)(void));
extern word read_tls_word(void);
static long call(long n, word a, word b, word c, word d) {
    return linux_call(n, a, b, c, d, 0, 0);
}
static _Atomic unsigned int tid, ready, done;
static _Atomic word allocation;
static long tgid, event;
static long signal_fd;
static _Atomic unsigned int *report;
static word tls[2] = { 0x12345678, 0 };
static int worker(void) {
    if (call(39, 0, 0, 0, 0) != tgid || call(186, 0, 0, 0, 0) != tid || read_tls_word() != tls[0]) { done = 11; return 11; }
    long memory = linux_call(9, 0, 4096, 3, 0x22, (word)-1, 0);
    if (memory < 0) { done = 12; return 12; }
    *(volatile word *)memory = 0xabcdef;
    allocation = memory;
    if (call(80, (word)"/tmp", 0, 0, 0) != 0) { done = 13; return 13; }
    if (call(95, 077, 0, 0, 0) != 022) { done = 17; return 17; }
    ready = 1;
    word count = 1;
    if (call(1, event, (word)&count, 8, 0) != 8) { done = 14; return 14; }
    call(202, (word)&ready, 129, 1, 0);
    while (ready == 1) {
        call(202, (word)&ready, 128, 1, 0);
    }
    if (*(volatile word *)allocation != 0x11223344) { done = 15; return 15; }
    if (call(3, event, 0, 0, 0) != 0) { done = 16; return 16; }
    done = 1;
    return 0;
}
static int signal_worker(void) {
    while (ready == 0) { call(202, (word)&ready, 128, 0, 0); }
    unsigned int records[64];
    long bytes = call(0, signal_fd, (word)records, sizeof(records), 0);
    if (bytes != 256 || records[0] != 10 || records[32] != 12) { done = 31; return 31; }
    done = 1;
    return 0;
}
static int orphan_worker(void) {
    word delay[2] = { 0, 100000000 };
    call(35, (word)delay, 0, 0, 0);
    if (call(39, 0, 0, 0, 0) != tgid) { *report = 41; return 41; }
    long memory = linux_call(9, 0, 4096, 3, 0x22, (word)-1, 0);
    if (memory < 0) { *report = 42; return 42; }
    *(volatile word *)memory = 0x98765432;
    if (call(10, memory, 4096, 1, 0) != 0 || *(volatile word *)memory != 0x98765432) { *report = 43; return 43; }
    if (call(11, memory, 4096, 0, 0) != 0) { *report = 44; return 44; }
    *report = 1;
    return 0;
}
static int group_exit_worker(void) {
    word delay[2] = { 0, 10000000 };
    call(35, (word)delay, 0, 0, 0);
    call(231, 77, 0, 0, 0);
    return 99;
}
int test_main(void) {
    tgid = call(39, 0, 0, 0, 0);
    event = call(290, 0, 0x800, 0, 0);
    long stack = linux_call(9, 0, 65536, 3, 0x22, (word)-1, 0);
    if (event < 0 || stack < 0) { return 20; }
    long child = thread_start(0x13d0f00, stack + 65536, (word)&tid, (word)&tid, (word)tls, worker);
    if (child < 0 || tid != (unsigned int)child) { return 21; }
    word timeout[2] = { 2, 0 };
    while (ready == 0 && done == 0) {
        long result = call(202, (word)&ready, 128, 0, (word)timeout);
        if (result != 0 && result != -11) { return 22; }
    }
    if (done != 0 || *(volatile word *)allocation != 0xabcdef) { return 23; }
    if (call(95, 022, 0, 0, 0) != 077) { return 27; }
    word count = 0;
    if (call(0, event, (word)&count, 8, 0) != 8 || count != 1) { return 24; }
    char cwd[16];
    if (call(79, (word)cwd, sizeof(cwd), 0, 0) != 5 || cwd[1] != 't') { return 25; }
    *(volatile word *)allocation = 0x11223344;
    ready = 2;
    call(202, (word)&ready, 129, 1, 0);
    while (tid != 0) {
        unsigned int observed = tid;
        if (observed != 0) {
            long result = call(202, (word)&tid, 128, observed, (word)timeout);
            if (result != 0 && result != -11) { return 26; }
        }
    }
    if (done != 1 || call(0, event, (word)&count, 8, 0) != -9) { return 27; }
    unsigned int status = 0;
    if (call(61, child, (word)&status, 1, 0) != -10) { return 28; }
    if (call(11, allocation, 4096, 0, 0) != 0 || call(11, stack, 65536, 0, 0) != 0) { return 29; }

    word mask = (1ul << 9) | (1ul << 11);
    call(14, 0, (word)&mask, 0, 8);
    signal_fd = call(289, (word)-1, (word)&mask, 8, 0x800);
    stack = linux_call(9, 0, 65536, 3, 0x22, (word)-1, 0);
    ready = 0; done = 0;
    child = thread_start(0x13d0f00, stack + 65536, (word)&tid, (word)&tid, (word)tls, signal_worker);
    if (child < 0 || signal_fd < 0 || call(62, tgid, 10, 0, 0) != 0 || call(234, tgid, child, 12, 0) != 0) { return 30; }
    ready = 1;
    call(202, (word)&ready, 129, 1, 0);
    while (tid != 0) {
        unsigned int observed = tid;
        if (observed != 0) { call(202, (word)&tid, 128, observed, (word)timeout); }
    }
    unsigned int record[32];
    if (done != 1 || call(0, signal_fd, (word)record, sizeof(record), 0) != -11) { return 32; }
    call(3, signal_fd, 0, 0, 0);
    call(14, 1, (word)&mask, 0, 8);
    call(11, stack, 65536, 0, 0);

    long shared = linux_call(9, 0, 4096, 3, 0x21, (word)-1, 0);
    if (shared < 0) { return 40; }
    report = (_Atomic unsigned int *)shared;
    child = call(57, 0, 0, 0, 0);
    if (child == 0) {
        tgid = call(39, 0, 0, 0, 0);
        stack = linux_call(9, 0, 65536, 3, 0x22, (word)-1, 0);
        if (thread_start(0x13d0f00, stack + 65536, (word)&tid, (word)&tid, (word)tls, orphan_worker) < 0) { return 45; }
        return 33; /* The VM must outlive its original task. */
    }
    status = 0;
    if (child < 0 || call(61, child, (word)&status, 0, 0) != child || status != (33 << 8) || *report != 1) { return 46; }
    call(11, shared, 4096, 0, 0);

    child = call(57, 0, 0, 0, 0);
    if (child == 0) {
        stack = linux_call(9, 0, 65536, 3, 0x22, (word)-1, 0);
        if (thread_start(0x13d0f00, stack + 65536, (word)&tid, (word)&tid, (word)tls, group_exit_worker) < 0) { return 47; }
        ready = 0;
        call(202, (word)&ready, 128, 0, 0);
        return 48;
    }
    status = 0;
    if (child < 0 || call(61, child, (word)&status, 0, 0) != child || status != (77 << 8)) { return 49; }
    static const char pass[] = "THREADS-PASS\n";
    call(1, 1, (word)pass, sizeof(pass) - 1, 0);
    return 0;
}
