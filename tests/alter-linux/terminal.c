/* x86-64 freestanding Linux fixture, run after the QMP BusyBox vi test. */
static long syscall3(long n, long a, long b, long c) {
    long result;
    __asm__ volatile("syscall" : "=a"(result) : "a"(n), "D"(a), "S"(b), "d"(c)
                     : "rcx", "r11", "memory");
    return result;
}
#define SC(n,a,b,c) syscall3(n,(long)(a),(long)(b),(long)(c))
#define CHECK(c) do { if (!(c)) return __LINE__; } while (0)
struct winsize { unsigned short rows, cols, x, y; };
struct termios { unsigned int iflag, oflag, cflag, lflag; unsigned char line, cc[19]; };
long test_main(void) {
    char contents[128];
    long fd = SC(2, "/tmp/vi-test", 0, 0);
    CHECK(fd >= 0);
    long bytes = SC(0, fd, contents, sizeof(contents));
    CHECK(bytes > 0);
    const char expected[] = "second line\n";
    CHECK(bytes == sizeof(expected) - 1);
    for (int i = 0; i < bytes; i++) CHECK(contents[i] == expected[i]);
    CHECK(SC(3, fd, 0, 0) == 0);
    fd = SC(2, "/tmp/vi-copy", 0, 0);
    CHECK(fd >= 0 && SC(0, fd, contents, sizeof(contents)) == sizeof(expected) - 1);
    for (int i = 0; i < sizeof(expected) - 1; ++i) CHECK(contents[i] == expected[i]);
    CHECK(SC(3, fd, 0, 0) == 0);
    struct winsize size;
    CHECK(SC(16, 0, 0x5413, &size) == 0);
    CHECK(size.cols == 101 && size.rows == 37);
    struct termios original, raw, observed;
    CHECK(SC(16, 0, 0x5401, &original) == 0);
    raw = original;
    raw.iflag = 0;
    raw.oflag = 0;
    raw.lflag &= ~(2U | 8U); /* ICANON | ECHO */
    raw.cc[6] = 1; /* VMIN */
    raw.cc[5] = 0; /* VTIME */
    CHECK(SC(16, 0, 0x5402, &raw) == 0);
    CHECK(SC(16, 0, 0x5401, &observed) == 0);
    for (int i = 0; i < sizeof(raw); i++) CHECK(((char *)&raw)[i] == ((char *)&observed)[i]);
    CHECK(SC(16, 0, 0x5402, &original) == 0);
    const char ok[] = "terminal fixture passed\n";
    CHECK(SC(1, 1, ok, sizeof(ok) - 1) == sizeof(ok) - 1);
    return 0;
}
__asm__(".global _start\n_start:\nxor %ebp,%ebp\nand $-16,%rsp\ncall test_main\nmov %rax,%rdi\nmov $60,%eax\nsyscall\nud2\n");
