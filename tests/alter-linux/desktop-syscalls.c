/* Integer-only regression of the Linux ABI used by the Tiny Core desktop. */
typedef unsigned long word;
extern long linux_call(long, word, word, word, word, word, word);
extern long clone3_child(void *, word, void (*)(void));
static long call(long n, word a, word b, word c, word d) {
    return linux_call(n, a, b, c, d, 0, 0);
}
#define CHECK(test) do { if (!(test)) { return __LINE__; } } while (0)
static int equal(const char *a, const char *b, word n) {
    for (word i = 0; i < n; ++i) {
        if (a[i] != b[i]) { return 0; }
    }
    return 1;
}
struct clone_args { word flags, pidfd, child_tid, parent_tid, signal, stack, size, tls, set_tid, set_tid_size, cgroup, tail; };
static volatile word shared;
enum { STACK_BYTES = 16384 };
static void shared_child(void) { shared = 0x1234; }

int test_main(void) {
    _Alignas(8) unsigned char system_info[120];
    for (word i = 0; i < sizeof(system_info); ++i) { system_info[i] = 0xa5; }
    word before[2], after[2];
    CHECK(call(228, 1, (word)before, 0, 0) == 0);
    CHECK(call(99, (word)system_info, 0, 0, 0) == 0);
    CHECK(call(228, 1, (word)after, 0, 0) == 0);
    CHECK(*(word *)system_info >= before[0] && *(word *)system_info <= after[0] + 1);
    CHECK(*(word *)(system_info + 32) > 0 && *(word *)(system_info + 40) <= *(word *)(system_info + 32));
    CHECK(*(unsigned short *)(system_info + 80) > 0 && *(unsigned int *)(system_info + 104) == 1);
    for (word i = 112; i < sizeof(system_info); ++i) { CHECK(system_info[i] == 0xa5); }
    for (word i = 82; i < 88; ++i) { CHECK(system_info[i] == 0); }
    for (word i = 108; i < 112; ++i) { CHECK(system_info[i] == 0); }
    CHECK(call(99, 0, 0, 0, 0) == -14 && call(99, 1, 0, 0, 0) == -14);
    long system_pages = linux_call(9, 0, 8192, 3, 0x22, (word)-1, 0);
    CHECK(system_pages > 0);
    CHECK(call(99, system_pages + 4096 - 56, 0, 0, 0) == 0);
    CHECK(call(10, system_pages + 4096, 4096, 1, 0) == 0);
    CHECK(call(99, system_pages + 4096 - 56, 0, 0, 0) == -14);
    CHECK(call(11, system_pages, 8192, 0, 0) == 0);
    CHECK(call(117, 0, (word)-1, 0, 0) == 0);
    CHECK(call(119, (word)-1, 0, 0, 0) == 0);
    CHECK(call(117, 1000, 0, 0, 0) == -95);
    CHECK(call(119, 0, 1000, 0, 0) == -95);
    static const char name[] = "tinycore-desktop-thread-name";
    char result[17] = {0};
    result[16] = 0x55;
    CHECK(call(157, 15, (word)name, 0, 0) == 0);
    CHECK(call(157, 16, (word)result, 0, 0) == 0);
    CHECK(equal(result, name, 15) && result[15] == 0 && result[16] == 0x55);
    CHECK(call(157, 15, 1, 0, 0) == -14);
    CHECK(call(157, 16, (word)result, 0, 0) == 0 && equal(result, name, 15));
    CHECK(call(157, 39, 0, 0, 0) == 0);
    CHECK(call(157, 38, 1, 0, 0) == 0);
    CHECK(call(157, 39, 0, 0, 0) == 1);
    CHECK(call(157, 38, 0, 0, 0) == -22);
    CHECK(call(157, 39, 1, 0, 0) == -22);
    CHECK(call(157, 0x7fffffff, 0, 0, 0) == -22);
    long comm = call(2, (word)"/proc/self/comm", 0, 0, 0);
    CHECK(comm >= 0 && call(0, comm, (word)result, 16, 0) == 16);
    CHECK(equal(result, name, 15) && result[15] == '\n');
    CHECK(call(3, comm, 0, 0, 0) == 0);

    word stats[16] = {0};
    stats[15] = 0x12345678;
    long fd = call(2, (word)"/tmp/syscalls-fs", 0102, 0600, 0);
    CHECK(fd >= 0 && call(138, fd, (word)stats, 0, 0) == 0);
    CHECK(call(1, fd, (word)"keep", 4, 0) == 4);
    for (word advice = 0; advice < 6; ++advice) {
        CHECK(call(221, fd, 0, 0, advice) == 0);
        CHECK(call(221, fd, (word)-1, 4096, advice) == 0);
    }
    CHECK(call(8, fd, 0, 1, 0) == 4);
    CHECK(call(8, fd, 0, 0, 0) == 0);
    char advised_contents[4];
    CHECK(call(0, fd, (word)advised_contents, 4, 0) == 4);
    CHECK(equal(advised_contents, "keep", 4));
    CHECK(call(221, fd, 0, (word)-1, 0) == -22);
    CHECK(call(221, fd, 0, 0, 6) == -22);
    CHECK(call(221, 100000, 0, 0, 0) == -9);
    int pipe_fds[2];
    CHECK(call(293, (word)pipe_fds, 0, 0, 0) == 0);
    CHECK(call(221, pipe_fds[0], 0, 0, 0) == -29);
    CHECK(call(221, pipe_fds[1], 0, (word)-1, 99) == -29);
    CHECK(call(3, pipe_fds[0], 0, 0, 0) == 0);
    CHECK(call(3, pipe_fds[1], 0, 0, 0) == 0);
    CHECK(stats[0] == 0xef53 && stats[1] == 1024 && stats[2] > stats[3]);
    CHECK(stats[3] >= stats[4] && stats[5] > stats[6] && stats[8] == 255);
    CHECK(stats[15] == 0x12345678);
    CHECK(call(138, fd, 1, 0, 0) == -14);
    CHECK(call(138, 100000, (word)stats, 0, 0) == -9);
    CHECK(call(137, (word)"/proc", (word)stats, 0, 0) == 0 && stats[0] == 0x9fa0);
    CHECK(call(3, fd, 0, 0, 0) == 0);
    CHECK(call(87, (word)"/tmp/syscalls-fs", 0, 0, 0) == 0);

    long id = call(29, 0x12345, 8193, 01000 | 0600, 0);
    CHECK(id > 0);
    CHECK(call(29, 0x12345, 0, 0, 0) == id);
    CHECK(call(29, 0x12345, 8193, 03000 | 0600, 0) == -17);
    CHECK(call(29, 0x12345, 16384, 0, 0) == -22);
    CHECK(call(29, 0, 0, 0600, 0) == -22);
    word info[15] = {0};
    info[14] = 0xabcdef;
    CHECK(call(31, id, 2, (word)info, 0) == 0 && info[6] == 8193 && info[11] == 0 && info[14] == 0xabcdef);
    CHECK(call(31, id, 2, 1, 0) == -14);
    long first = call(30, id, 0, 0, 0);
    long second = call(30, id, 0, 010000, 0);
    CHECK(first > 0 && second > 0 && first != second);
    volatile unsigned char *a = (void *)first;
    volatile unsigned char *b = (void *)second;
    CHECK(a[8192] == 0 && b[12287] == 0);
    a[8192] = 0x73;
    CHECK(b[8192] == 0x73);
    CHECK(call(10, second, 4096, 3, 0) == -13);
    CHECK(call(30, id, first, 0, 0) == -22 && a[8192] == 0x73);
    CHECK(call(31, id, 2, (word)info, 0) == 0 && info[11] == 2);

    struct clone_args args = {0};
    args.signal = 17;
    CHECK(call(435, (word)&args, 63, 0, 0) == -22);
    CHECK(call(435, 1, 88, 0, 0) == -14);
    args.tail = 1;
    CHECK(call(435, (word)&args, sizeof(args), 0, 0) == -7);
    args.tail = 0;
    args.flags = 17;
    CHECK(call(435, (word)&args, 88, 0, 0) == -22);
    args.flags = 0;
    long child = call(435, (word)&args, 88, 0, 0);
    CHECK(child >= 0);
    if (child == 0) {
        CHECK(call(157, 39, 0, 0, 0) == 1);
        CHECK(call(157, 16, (word)result, 0, 0) == 0 && equal(result, name, 15));
        CHECK(call(31, id, 2, (word)info, 0) == 0 && info[11] == 4);
        a[8192] = 0x74;
        CHECK(call(67, first, 0, 0, 0) == 0);
        return 0;
    }
    int status = -1;
    CHECK(call(61, child, (word)&status, 0, 0) == child && status == 0);
    CHECK(a[8192] == 0x74 && b[8192] == 0x74);
    CHECK(call(31, id, 2, (word)info, 0) == 0 && info[11] == 2);
    CHECK(call(31, id, 0, 0, 0) == 0);
    CHECK(call(29, 0x12345, 0, 0, 0) == -2);
    CHECK(call(31, id, 2, (word)info, 0) == 0 && info[11] == 2);
    /* Detach the remaining pieces, without discarding a replacement VMA. */
    CHECK(call(11, first + 4096, 4096, 0, 0) == 0);
    CHECK(linux_call(9, first + 4096, 4096, 3, 0x32, (word)-1, 0) == first + 4096);
    volatile char *replacement = (void *)(first + 4096);
    *replacement = 0x45;
    CHECK(call(67, first, 0, 0, 0) == 0 && *replacement == 0x45);
    CHECK(call(67, second, 0, 0, 0) == 0);
    CHECK(call(31, id, 2, (word)info, 0) == -22);
    CHECK(call(67, second, 0, 0, 0) == -22);
    CHECK(call(11, (word)replacement, 4096, 0, 0) == 0);
    /* Repeated allocate/remove cycles must reuse the object and page budget. */
    for (int i = 0; i < 270; ++i) {
        long fresh = call(29, 0, 4096, 0600, 0);
        CHECK(fresh > 0 && call(31, fresh, 0, 0, 0) == 0);
    }

    args.flags = 0x100; /* CLONE_VM with a separate thread stack. */
    long guarded = linux_call(9, 0, STACK_BYTES + 4096, 3, 0x22, (word)-1, 0);
    CHECK(guarded > 0 && call(10, guarded, 4096, 0, 0) == 0);
    args.stack = guarded;
    args.size = STACK_BYTES + 4096;
    child = clone3_child(&args, 88, shared_child);
    CHECK(child > 0);
    CHECK(call(61, child, (word)&status, 0, 0) == child && status == 0 && shared == 0x1234);
    CHECK(call(11, guarded, STACK_BYTES + 4096, 0, 0) == 0);
    /* No shared address space is silently replaced by a private vfork copy. */
    args.flags |= 0x4000;
    CHECK(call(435, (word)&args, 88, 0, 0) == -38);
    static const char pass[] = "DESKTOP-SYSCALLS-PASS\n";
    call(1, 1, (word)pass, sizeof(pass) - 1, 0);
    return 0;
}
