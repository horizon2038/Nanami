/* Freestanding x86-64 Linux ftruncate regression; only writes /tmp/truncate-test. */
static long sc(long n, long a, long b, long c) {
    long result;
    __asm__ volatile("syscall" : "=a"(result) : "a"(n), "D"(a), "S"(b), "d"(c)
                     : "rcx", "r11", "memory");
    return result;
}
#define SC(n,a,b,c) sc(n,(long)(a),(long)(b),(long)(c))
#define CHECK(c) do { if (!(c)) return __LINE__; } while (0)
static unsigned char data[4096], output[4096];
struct stat64 {
    unsigned long dev, ino, nlink;
    unsigned int mode, uid, gid, pad;
    unsigned long rdev;
    long size, blksize, blocks, atime, atime_ns, mtime, mtime_ns, ctime, ctime_ns, reserved[3];
};
long test_main(void) {
    const char *path = "/tmp/truncate-test";
    long fd = SC(2, path, 2 | 64 | 512, 0600);
    CHECK(fd >= 0);
    long other = SC(2, path, 0, 0), dup = SC(32, fd, 0, 0);
    CHECK(other >= 0 && dup >= 0);
    CHECK(SC(77, -1, 1, 0) == -9);
    CHECK(SC(77, other, 1, 0) == -22);
    CHECK(SC(77, fd, -1, 0) == -22);
    CHECK(SC(77, fd, 0x100000000L, 0) < 0);
    long directory = SC(2, "/tmp", 0, 0);
    CHECK(directory >= 0 && SC(77, directory, 0, 0) < 0);
    CHECK(SC(3, directory, 0, 0) == 0);
    for (int i = 0; i < sizeof(data); ++i) data[i] = (i % 251) + 1;
    /* Cross both indirect levels (12 + 256 data blocks at 1 KiB each). */
    const long total = 600 * 1024;
    for (long offset = 0; offset < total; offset += sizeof(data))
        CHECK(SC(1, fd, data, sizeof(data)) == sizeof(data));
    const long lengths[] = { total, 300 * 1024 + 7, 268 * 1024, 12 * 1024 + 9, 12 * 1024, 7, 0 };
    for (int test = 0; test < sizeof(lengths) / sizeof(*lengths); ++test) {
        long length = lengths[test];
        CHECK(SC(8, fd, 123, 0) == 123);
        CHECK(SC(77, dup, length, 0) == 0);
        CHECK(SC(8, fd, 0, 1) == 123);
        struct stat64 status;
        CHECK(SC(5, other, &status, 0) == 0 && status.size == length);
        CHECK(SC(8, other, 0, 0) == 0);
        for (long offset = 0; offset < length;) {
            long bytes = length - offset < sizeof(output) ? length - offset : sizeof(output);
            CHECK(SC(0, other, output, bytes) == bytes);
            for (int i = 0; i < bytes; ++i) CHECK(output[i] == data[(offset + i) % sizeof(data)]);
            offset += bytes;
        }
        CHECK(SC(0, other, output, 1) == 0);
    }
    /* No old bytes may reappear after a sparse extension. */
    CHECK(SC(77, fd, total, 0) == 0);
    CHECK(SC(8, other, 0, 0) == 0);
    for (long offset = 0; offset < total; offset += sizeof(output)) {
        CHECK(SC(0, other, output, sizeof(output)) == sizeof(output));
        for (int i = 0; i < sizeof(output); ++i) CHECK(output[i] == 0);
    }
    CHECK(SC(8, fd, total - 3, 0) == total - 3);
    CHECK(SC(1, fd, "end", 3) == 3);
    CHECK(SC(8, other, total - 8, 0) == total - 8);
    CHECK(SC(0, other, output, 8) == 8);
    for (int i = 0; i < 5; ++i) CHECK(output[i] == 0);
    CHECK(output[5] == 'e' && output[6] == 'n' && output[7] == 'd');
    CHECK(SC(8, fd, 0, 0) == 0 && SC(1, fd, "hello stale tail", 16) == 16);
    CHECK(SC(77, fd, 5, 0) == 0 && SC(77, fd, 20, 0) == 0);
    CHECK(SC(74, fd, 0, 0) == 0);
    CHECK(SC(3, dup, 0, 0) == 0 && SC(3, other, 0, 0) == 0 && SC(3, fd, 0, 0) == 0);
    fd = SC(2, path, 0, 0);
    CHECK(fd >= 0 && SC(0, fd, output, sizeof(output)) == 20);
    for (int i = 0; i < 20; ++i) CHECK(output[i] == (i < 5 ? "hello"[i] : 0));
    CHECK(SC(3, fd, 0, 0) == 0);
    const char ok[] = "ftruncate fixture passed\n";
    CHECK(SC(1, 1, ok, sizeof(ok) - 1) == sizeof(ok) - 1);
    return 0;
}
__asm__(".global _start\n_start:\nxor %ebp,%ebp\nand $-16,%rsp\ncall test_main\nmov %rax,%rdi\nmov $60,%eax\nsyscall\nud2\n");
