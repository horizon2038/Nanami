/* Freestanding x86-64 Linux time(2) ABI regression. No libc/vDSO shortcut. */
typedef unsigned long usize;
struct timespec { long sec, nsec; };

static long syscall4(long number, long a, long b, long c, long d) {
    register long r10 __asm__("r10") = d;
    long result;
    __asm__ volatile("syscall" : "=a"(result)
                     : "a"(number), "D"(a), "S"(b), "d"(c), "r"(r10)
                     : "rcx", "r11", "memory");
    return result;
}
#define sc(n,a,b,c) syscall4(n,(long)(a),(long)(b),(long)(c),0)
#define sc4(n,a,b,c,d) syscall4(n,(long)(a),(long)(b),(long)(c),(long)(d))

static usize monotonic_ns(void) {
    struct timespec now;
    if (sc(228, 1, &now, 0) != 0) {
        return 0;
    }
    return (usize)now.sec * 1000000000 + now.nsec;
}

static long clock_sleep_test(void) {
    const long clocks[] = { 0, 1, 7 };
    for (usize i = 0; i < 3; ++i) {
        struct timespec request = { 0, 2000000 };
        usize before = monotonic_ns();
        if (sc4(230, clocks[i], 0, &request, 1) != 0 || monotonic_ns() - before < 2000000) {
            return 7;
        }
        struct timespec target, after;
        if (sc(228, clocks[i], &target, 0) != 0) {
            return 8;
        }
        target.nsec += 2000000;
        if (target.nsec >= 1000000000) {
            ++target.sec;
            target.nsec -= 1000000000;
        }
        if (sc4(230, clocks[i], 1, &target, 1) != 0 || sc(228, clocks[i], &after, 0) != 0
            || after.sec < target.sec || (after.sec == target.sec && after.nsec < target.nsec)) {
            return 9;
        }
        struct timespec past = { 0, 0 };
        if (sc4(230, clocks[i], 1, &past, 1) != 0) {
            return 10;
        }
    }
    struct timespec bad = { -1, 0 };
    if (sc4(230, 1, 0, &bad, 0) != -22 || sc4(230, 3, 0, &bad, 0) != -22
        || sc4(230, 4, 0, &bad, 0) != -95 || sc4(230, 1, 0, 0, 0) != -14
        || sc4(230, 1, 0, 1, 0) != -14) {
        return 11;
    }
    return 0;
}

long test_main(void) {
    long before = sc(201, 0, 0, 0);
    if (before < 946684800) {
        return 1; /* RTC epoch, not uptime or ENOSYS. */
    }
    long stored = -1;
    long result = sc(201, &stored, 0, 0);
    if (result != stored || result < before) {
        return 2;
    }
    unsigned char unaligned[10];
    for (usize i = 0; i < sizeof(unaligned); ++i) {
        unaligned[i] = 0xa5;
    }
    result = sc(201, unaligned + 1, 0, 0);
    usize decoded = 0;
    for (usize i = 0; i < 8; ++i) {
        decoded |= (usize)unaligned[i + 1] << (i * 8);
    }
    if (result < before || decoded != (usize)result || unaligned[0] != 0xa5 || unaligned[9] != 0xa5) {
        return 3;
    }
    if (sc(201, 1, 0, 0) != -14 || sc(201, ~(usize)0, 0, 0) != -14) {
        return 4;
    }
    struct timespec realtime;
    if (sc(228, 0, &realtime, 0) != 0 || realtime.sec < result || realtime.sec - result > 2) {
        return 5;
    }
    struct timespec delay = { 1, 50000000 };
    if (sc(35, &delay, 0, 0) != 0 || sc(201, 0, 0, 0) <= result) {
        return 6;
    }
    return clock_sleep_test();
}

__asm__(".global _start\n_start:\nxor %ebp,%ebp\nand $-16,%rsp\ncall test_main\nmov %rax,%rdi\nmov $60,%eax\nsyscall\nud2\n");
