/* Freestanding x86-64 Linux clock/sleep regression fixture.
 * Link with -Wl,--image-base=0x400000: Alter's current fork ELF-header lookup
 * uses LINUX_IMAGE_BASE (unrelated to the timer implementation).
 */
typedef unsigned long usize;
struct timespec { long sec, nsec; };
static long syscall4(long n, long a, long b, long c, long d) {
    register long r10 __asm__("r10") = d;
    long result;
    __asm__ volatile("syscall" : "=a"(result) : "a"(n), "D"(a), "S"(b), "d"(c), "r"(r10)
                     : "rcx", "r11", "memory");
    return result;
}
#define sc(n,a,b,c) syscall4(n,(long)(a),(long)(b),(long)(c),0)
static usize now(void) {
    struct timespec ts;
    if (sc(228,1,&ts,0) != 0 || ts.sec < 0 || ts.nsec < 0 || ts.nsec >= 1000000000)
        return ~(usize)0;
    return (usize)ts.sec * 1000000000 + ts.nsec;
}
static long sleep_test(usize ns) {
    struct timespec request = { ns / 1000000000, ns % 1000000000 };
    usize before = now();
    if (before == ~(usize)0 || sc(35,&request,0,0) != 0) return 1;
    usize after = now();
    return after == ~(usize)0 || after < before || after - before < ns;
}
long test_main(void) {
    usize before = now(), last = before;
    if (before == ~(usize)0) return 1;
    for (int i=0; i<128; ++i) {
        usize ticks = now();
        if (ticks == ~(usize)0 || ticks < last) return 2;
        last = ticks;
    }
    if (last == before) return 3;
    struct timespec bad = { -1, 0 }, zero = { 0, 0 };
    if (sc(35,&bad,0,0) != -22 || sc(35,&zero,0,0) != 0) return 4;
    bad.sec = 0; bad.nsec = 1000000000;
    if (sc(35,&bad,0,0) != -22) return 5;
    for (int i=0; i<16; ++i) if (sleep_test(1000000)) return 6;
    if (sleep_test(100000000)) return 7;
    long children[2];
    for (int i=0; i<2; ++i) {
        long pid = sc(57,0,0,0);
        if (pid < 0) return 8;
        if (pid == 0) {
            long status = sleep_test(i == 0 ? 300000000 : 1000000);
            sc(60,status,0,0);
            __builtin_unreachable();
        }
        children[i] = pid;
    }
    for (int i=0; i<2; ++i) {
        int status = -1;
        if (sc(61,children[i],&status,0) != children[i] || status != 0) return 9;
    }
    return 0;
}
__asm__(".global _start\n_start:\nxor %ebp,%ebp\nand $-16,%rsp\ncall test_main\nmov %rax,%rdi\nmov $60,%eax\nsyscall\nud2\n");
