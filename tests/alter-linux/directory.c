/* Freestanding x86-64 Linux regression: link at 0x400000 for Alter's fork path.
 * Run only on a disposable QEMU snapshot; creates files in the guest root. */
static long sc(long n, long a, long b, long c) {
    long result;
    register long fourth __asm__("r10") = 0;
    __asm__ volatile("syscall" : "=a"(result) : "a"(n), "D"(a), "S"(b), "d"(c), "r"(fourth)
                     : "rcx", "r11", "memory");
    return result;
}
#define SC(n,a,b,c) sc(n,(long)(a),(long)(b),(long)(c))
#define CHECK(c) do { if (!(c)) return __LINE__; } while (0)
static char output[8192];
static int contains(const char *text) {
    for (int i = 0; output[i]; ++i) {
        int j = 0;
        while (text[j] && output[i+j] == text[j]) ++j;
        if (!text[j]) return 1;
    }
    return 0;
}
static int equal(const char *a, const char *b) {
    while (*a && *a == *b) { ++a; ++b; }
    return *a == *b;
}
static unsigned char entries[4096];
static int list(long fd, int capacity, const char *name) {
    int found = 0;
    for (int batch = 0; batch < 500; ++batch) {
        long bytes = SC(217, fd, entries, capacity);
        if (bytes < 0) return -1;
        if (!bytes) return found;
        for (long offset = 0; offset < bytes;) {
            unsigned short size = *(unsigned short *)(entries + offset + 16);
            if (size < 24 || size % 8 || offset + size > bytes) return -2;
            if (equal((char *)entries + offset + 19, name)) ++found;
            offset += size;
        }
    }
    return -3;
}
static int bash(const char *script) {
    long child = SC(57, 0, 0, 0);
    if (child == 0) {
        long fd = SC(2, "/tmp/directory-test-output", 2 | 64 | 512, 0600);
        if (fd < 0) SC(60, 120, 0, 0);
        SC(33, fd, 1, 0); SC(33, fd, 2, 0); SC(3, fd, 0, 0);
        const char *argv[] = { "bash", "--noprofile", "--norc", "-c", script, 0 };
        const char *env[] = { "PATH=/bin:/usr/bin", "TERM=vt100", 0 };
        SC(59, "/bin/bash", argv, env);
        SC(60, 121, 0, 0);
    }
    if (child < 0) return -1;
    int status = -1;
    if (SC(61, child, &status, 0) != child) return -2;
    long fd = SC(2, "/tmp/directory-test-output", 0, 0);
    if (fd < 0) return -3;
    long bytes = SC(0, fd, output, sizeof(output) - 1);
    SC(3, fd, 0, 0);
    if (bytes < 0) return -4;
    output[bytes] = 0;
    SC(1, 1, output, bytes);
    return status;
}
long test_main(void) {
    CHECK(bash("cd / && printf 'saved-content\n' > listing-save && "
               "/bin/busybox cat listing-save && /bin/busybox ls -1 .") == 0);
    CHECK(contains("saved-content\n"));
    CHECK(contains("listing-save\n"));
    CHECK(contains("dev\n") && contains("proc\n") && contains("sys\n"));
    long root = SC(2, "/", 65536, 0);
    CHECK(root >= 0);
    CHECK(SC(217, root, entries, 1) == -22);
    CHECK(SC(217, root, entries, 24) == 24 && equal((char *)entries + 19, "dev"));
    long cookie = *(long *)(entries + 8);
    long copy = SC(32, root, 0, 0);
    CHECK(copy >= 0);
    CHECK(SC(217, copy, entries, 24) == 24 && equal((char *)entries + 19, "proc"));
    CHECK(SC(217, root, entries, 24) == 24 && equal((char *)entries + 19, "sys"));
    CHECK(SC(8, copy, cookie, 0) == cookie);
    CHECK(SC(217, root, entries, 24) == 24 && equal((char *)entries + 19, "proc"));
    CHECK(SC(3, root, 0, 0) == 0 && SC(8, copy, 0, 0) == 0);
    CHECK(list(copy, 32, "listing-save") == 1);
    CHECK(SC(8, copy, 0, 0) == 0 && list(copy, 32, "dev") == 1);
    CHECK(SC(3, copy, 0, 0) == 0);

    CHECK(SC(83, "/listing-dir", 0755, 0) == 0);
    for (int i = 0; i < 40; ++i) {
        char path[] = "/listing-dir/file-00";
        path[19] = '0' + i % 10; path[18] = '0' + i / 10;
        long fd = SC(2, path, 1 | 64 | 512, 0600);
        CHECK(fd >= 0 && SC(3, fd, 0, 0) == 0);
    }
    long dir = SC(2, "/listing-dir", 65536, 0);
    CHECK(dir >= 0 && list(dir, 32, "file-39") == 1);
    CHECK(SC(8, dir, 0, 0) == 0 && list(dir, 32, "file-00") == 1);
    CHECK(SC(3, dir, 0, 0) == 0);
    CHECK(bash("cd / && /bin/busybox mv listing-save renamed-save && "
               "/bin/busybox ls -1 .") == 0);
    CHECK(contains("renamed-save\n") && !contains("listing-save\n"));
    CHECK(bash("cd / && /bin/busybox rm renamed-save && /bin/busybox ls -1 .") == 0);
    CHECK(!contains("renamed-save\n"));
    const char ok[] = "directory fixture passed\n";
    SC(1, 1, ok, sizeof(ok) - 1);
    return 0;
}
__asm__(".global _start\n_start:\nxor %ebp,%ebp\nand $-16,%rsp\ncall test_main\nmov %rax,%rdi\nmov $60,%eax\nsyscall\nud2\n");
