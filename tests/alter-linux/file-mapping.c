/* Shared aliases, private initialization and fork of a deleted executable. */
typedef unsigned long word;
extern long linux_call(long, word, word, word, word, word, word);
static long call(long n, word a, word b, word c, word d) {
    return linux_call(n, a, b, c, d, 0, 0);
}
_Alignas(4096) static volatile unsigned char protected_state[4096];
static int private_mapping_and_fork(void) {
    static unsigned char data[5003];
    for (word i = 0; i < sizeof(data); ++i) { data[i] = (unsigned char)(i % 239); }
    const char *name = "/tmp/private-file-test";
    long fd = call(2, (word)name, 0102, 0600, 0);
    if (fd < 0 || call(1, fd, (word)data, sizeof(data), 0) != sizeof(data)) { return 40; }
    long first = linux_call(9, 0, 8192, 3, 2, fd, 0);
    long second = linux_call(9, 0, 8192, 1, 2, fd, 0);
    if (first < 0 || second < 0) { return 41; }
    volatile unsigned char *a = (void *)first;
    volatile unsigned char *b = (void *)second;
    for (word i = 0; i < 8192; ++i) {
        unsigned char expected = 0;
        if (i < sizeof(data)) { expected = data[i]; }
        if (a[i] != expected || b[i] != expected) { return 42; }
    }
    a[0] = 0x51;
    protected_state[0] = 0x37;
    if (call(10, (word)protected_state, 4096, 1, 0) != 0
        || call(10, first, 8192, 0, 0) != 0) { return 43; }
    /* The running image is removed only from this QEMU snapshot. Fork must
       clone the current VM rather than reopen / reload the executable. */
    if (call(87, (word)"/bin/nanami-file-mapping", 0, 0, 0) != 0) { return 44; }
    long child = call(57, 0, 0, 0, 0);
    if (child < 0) { return 45; }
    if (child == 0) {
        if (call(10, first, 8192, 3, 0) != 0 || a[0] != 0x51
            || b[1] != data[1] || a[5003] != 0 || a[8191] != 0
            || protected_state[0] != 0x37) { return 46; }
        a[0] = 0x52;
        call(60, 0, 0, 0, 0);
        return 57;
    }
    int status = -1;
    if (call(61, child, (word)&status, 0, 0) != child || status != 0) { return 47; }
    if (call(10, first, 8192, 3, 0) != 0 || a[0] != 0x51
        || protected_state[0] != 0x37) { return 48; }
    /* Replace old dirty backing and check the partial EOF page again. */
    if (linux_call(9, first, 8192, 1, 2 | 0x10, fd, 0) != first
        || a[0] != data[0] || a[5002] != data[5002] || a[8191] != 0) { return 49; }
    if (call(11, first, 8192, 0, 0) != 0 || call(11, second, 8192, 0, 0) != 0) { return 50; }
    call(3, fd, 0, 0, 0);
    call(87, (word)name, 0, 0, 0);
    return 0;
}
static int uncached_private_mapping(void) {
    static unsigned char block[65536];
    for (word i = 0; i < sizeof(block); ++i) { block[i] = 0x4d; }
    const char *name = "/tmp/large-private-file-test";
    long fd = call(2, (word)name, 0102, 0600, 0);
    if (fd < 0) { return 51; }
    for (word i = 0; i < 32; ++i) {
        if (call(1, fd, (word)block, sizeof(block), 0) != sizeof(block)) { return 52; }
    }
    if (call(1, fd, (word)block, 123, 0) != 123) { return 53; }
    word bytes = 2 * 1024 * 1024;
    long address = linux_call(9, 0, bytes + 4096, 1, 2, fd, 0);
    if (address < 0) { return 54; }
    volatile unsigned char *a = (void *)address;
    for (word i = 0; i < bytes + 4096; ++i) {
        unsigned char expected = 0;
        if (i < bytes + 123) { expected = 0x4d; }
        if (a[i] != expected) { return 55; }
    }
    if (call(11, address, bytes + 4096, 0, 0) != 0) { return 56; }
    call(3, fd, 0, 0, 0);
    call(87, (word)name, 0, 0, 0);
    return 0;
}
int test_main(void) {
    static char data[8192];
    for (word i = 0; i < sizeof(data); ++i) { data[i] = (char)(i % 121); }
    const char *name = "/tmp/shared-file-test";
    long fd = call(2, (word)name, 0102, 0600, 0);
    if (fd < 0 || call(1, fd, (word)data, sizeof(data), 0) != sizeof(data)) { return 10; }
    long ro = call(2, (word)name, 0, 0, 0);
    long first = linux_call(9, 0, 5000, 1, 1, ro, 0);
    long other = linux_call(9, 0, 4096, 1, 1, fd, 4096);
    if (first < 0 || other < 0) { return 11; }
    volatile unsigned char *a = (void *)first;
    volatile unsigned char *b = (void *)other;
    if (a[4097] != (unsigned char)data[4097] || b[1] != a[4097]) { return 12; }
    if (call(3, ro, 0, 0, 0) != 0) { return 13; }
    if (call(10, first, 4096, 3, 0) != -13) { return 14; }
    if (linux_call(9, 0, 4096, 3, 1, fd, 0) != -95) { return 15; }
    char value = 0x7b;
    if (call(18, fd, (word)&value, 1, 4097) != 1 || a[4097] != 0x7b || b[1] != 0x7b) { return 16; }
    if (call(77, fd, 0, 0, 0) != -16) { return 17; }
    long child = call(57, 0, 0, 0, 0);
    if (child < 0) { return 18; }
    if (child == 0) {
        if (a[4097] != 0x7b || b[1] != 0x7b) { return 19; }
        value = 0x7c;
        if (call(18, fd, (word)&value, 1, 4097) != 1) { return 20; }
        return 0;
    }
    int status = -1;
    if (call(61, child, (word)&status, 0, 0) != child || status != 0 || a[4097] != 0x7c || b[1] != 0x7c) { return 21; }
    if (call(26, first, 5000, 4, 0) != 0) { return 30; }
    if (call(87, (word)name, 0, 0, 0) != 0 || call(3, fd, 0, 0, 0) != 0) { return 22; }
    if (call(11, first, 4096, 0, 0) != 0 || a[4097] != 0x7c) { return 23; }
    if (call(11, first + 4096, 4096, 0, 0) != 0 || b[1] != 0x7c) { return 24; }
    if (call(11, other, 4096, 0, 0) != 0) { return 25; }
    /* After the last unmap, the hidden FD and VFS page leases must be gone. */
    fd = call(2, (word)name, 0102, 0600, 0);
    if (fd < 0 || call(77, fd, 5000, 0, 0) != 0) { return 26; }
    first = linux_call(9, 0, 5000, 1, 1, fd, 0);
    if (first < 0) { return 27; }
    a = (void *)first;
    if (a[4999] != 0 || a[5000] != 0 || a[8191] != 0) { return 28; }
    if (call(11, first, 8192, 0, 0) != 0 || call(77, fd, 0, 0, 0) != 0) { return 29; }
    call(3, fd, 0, 0, 0);
    call(87, (word)name, 0, 0, 0);
    int result = uncached_private_mapping();
    if (result != 0) { return result; }
    result = private_mapping_and_fork();
    if (result != 0) { return result; }
    static const char pass[] = "FILE-MAPPING-PASS\n";
    call(1, 1, (word)pass, sizeof(pass) - 1, 0);
    return 0;
}
