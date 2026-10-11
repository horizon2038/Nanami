/* Linux sendfile ABI regression; link at 0x400000 and run on a snapshot. */
static long sc(long n, long a, long b, long c, long d) {
    long result;
    register long fourth __asm__("r10") = d;
    __asm__ volatile("syscall" : "=a"(result) : "a"(n), "D"(a), "S"(b), "d"(c), "r"(fourth)
                     : "rcx", "r11", "memory");
    return result;
}
#define SC(n,a,b,c,d) sc(n,(long)(a),(long)(b),(long)(c),(long)(d))
#define CHECK(c) do { if (!(c)) return __LINE__; } while (0)
static char data[100000], result[100000];
long test_main(void) {
    for (long i = 0; i < sizeof(data); ++i) data[i] = (char)(i * 17);
    long in = SC(2, "/tmp/sendfile-in", 2 | 64 | 512, 0600, 0);
    long out = SC(2, "/tmp/sendfile-out", 2 | 64 | 512, 0600, 0);
    CHECK(in >= 0 && out >= 0);
    CHECK(SC(1, in, data, sizeof(data), 0) == sizeof(data));
    CHECK(SC(8, in, 0, 0, 0) == 0);
    long offset = 7;
    CHECK(SC(40, out, in, &offset, 5) == 5 && offset == 12);
    CHECK(SC(8, in, 0, 1, 0) == 0); /* explicit offset preserves input position */
    CHECK(SC(8, out, 0, 0, 0) == 0 && SC(0, out, result, 5, 0) == 5);
    for (int i = 0; i < 5; ++i) CHECK(result[i] == data[i + 7]);
    offset = -1;
    CHECK(SC(40, out, in, &offset, 5) == -22);
    CHECK(SC(40, out, -1, 0, 5) == -9);
    CHECK(SC(40, out, in, 1, 5) == -14);
    CHECK(SC(8, out, 0, 0, 0) == 0);
    long done = 0;
    while (done < sizeof(data)) {
        long bytes = SC(40, out, in, 0, sizeof(data) - done);
        CHECK(bytes > 0 && bytes <= sizeof(data) - done);
        done += bytes;
    }
    CHECK(SC(40, out, in, 0, 1) == 0); /* EOF */
    CHECK(SC(8, in, 0, 1, 0) == sizeof(data));
    CHECK(SC(8, out, 0, 0, 0) == 0);
    done = 0;
    while (done < sizeof(data)) {
        long bytes = SC(0, out, result + done, sizeof(data) - done, 0);
        CHECK(bytes > 0); done += bytes;
    }
    for (long i = 0; i < sizeof(data); ++i) CHECK(data[i] == result[i]);
    CHECK(SC(72, out, 4, 1024, 0) == 0); /* shared O_APPEND */
    offset = 0;
    CHECK(SC(40, out, in, &offset, 5) == -22 && offset == 0);
    CHECK(SC(3, out, 0, 0, 0) == 0);
    out = SC(2, "/tmp/sendfile-out", 0, 0, 0);
    CHECK(out >= 0 && SC(40, out, in, &offset, 5) == -9);
    CHECK(SC(3, out, 0, 0, 0) == 0);
    int pipe[2];
    CHECK(SC(22, pipe, 0, 0, 0) == 0);
    CHECK(SC(40, pipe[1], in, &offset, 5) == 5 && offset == 5);
    CHECK(SC(0, pipe[0], result, 5, 0) == 5);
    for (int i = 0; i < 5; ++i) CHECK(result[i] == data[i]);
    CHECK(SC(3, pipe[0], 0, 0, 0) == 0 && SC(3, pipe[1], 0, 0, 0) == 0);
    CHECK(SC(3, in, 0, 0, 0) == 0);
    in = SC(2, "/tmp/sendfile-terminal", 2 | 64 | 512, 0600, 0);
    const char ok[] = "sendfile fixture passed\n";
    CHECK(in >= 0 && SC(1, in, ok, sizeof(ok) - 1, 0) == sizeof(ok) - 1);
    offset = 0;
    CHECK(SC(40, 1, in, &offset, sizeof(ok) - 1) == sizeof(ok) - 1);
    CHECK(offset == sizeof(ok) - 1 && SC(3, in, 0, 0, 0) == 0);
    return 0;
}
__asm__(".global _start\n_start:\nxor %ebp,%ebp\nand $-16,%rsp\ncall test_main\nmov %rax,%rdi\nmov $60,%eax\nsyscall\nud2\n");
