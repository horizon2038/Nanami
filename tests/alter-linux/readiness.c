/* Freestanding x86-64 Linux regression: real traps, pipe events and one-shot timeouts. */
typedef unsigned long word;
struct timespec { long sec, nsec; };
struct pollfd { int fd; short events, revents; };
static long sc(long n, long a, long b, long c, long d, long e, long f) {
    register long r10 __asm__("r10") = d;
    register long r8 __asm__("r8") = e;
    register long r9 __asm__("r9") = f;
    long result;
    __asm__ volatile("syscall" : "=a"(result) : "a"(n), "D"(a), "S"(b), "d"(c),
                     "r"(r10), "r"(r8), "r"(r9) : "rcx", "r11", "memory");
    return result;
}
#define s3(n,a,b,c) sc(n,(long)(a),(long)(b),(long)(c),0,0,0)
#define CHECK(x) do { if (!(x)) return __LINE__; } while (0)
static word now(void) {
    struct timespec ts;
    if (s3(228,1,&ts,0) != 0) return 0;
    return ts.sec * 1000000000UL + ts.nsec;
}
long test_main(void) {
    struct pollfd tty = {0, 1, -1};
    CHECK(s3(7,&tty,1,0) == 0 && tty.revents == 0);
    struct pollfd ignored = {-1, -1, -1};
    CHECK(s3(7,&ignored,1,0) == 0 && ignored.revents == 0);
    ignored.fd = 999;
    CHECK(s3(7,&ignored,1,0) == 1 && ignored.revents == 32);
    word before = now();
    CHECK(s3(7,0,0,25) == 0 && now() - before >= 25000000);
    struct timespec ts = {0, 15000000};
    before = now();
    CHECK(sc(271,0,0,(long)&ts,0,0,0) == 0);
    CHECK(now() - before >= 15000000 && ts.sec == 0 && ts.nsec == 0);
    ts.nsec = -1;
    CHECK(sc(271,0,0,(long)&ts,0,0,0) == -22);
    word reads = 1UL << 63;
    ts.nsec = 0;
    CHECK(sc(23,64,(long)&reads,0,0,(long)&ts,0) == -9);
    for (int mode = 0; mode < 3; ++mode) {
        int pipes[2];
        CHECK(s3(22,pipes,0,0) == 0);
        struct pollfd p = {pipes[0], 1, -1};
        CHECK(s3(7,&p,1,0) == 0 && p.revents == 0);
        long child = s3(57,0,0,0);
        CHECK(child >= 0);
        if (child == 0) {
            struct timespec delay = {0, 40000000};
            int status = s3(35,&delay,0,0) != 0 || s3(1,pipes[1],"x",1) != 1;
            s3(3,pipes[0],0,0);
            s3(3,pipes[1],0,0);
            s3(60,status,0,0);
            __builtin_unreachable();
        }
        CHECK(s3(3,pipes[1],0,0) == 0);
        if (mode == 0) {
            CHECK(s3(7,&p,1,-1) == 1 && (p.revents & 1));
        } else if (mode == 1) {
            ts.sec = 2; ts.nsec = 0;
            CHECK(sc(271,(long)&p,1,(long)&ts,0,0,0) == 1 && (p.revents & 1));
            CHECK(ts.sec < 2);
        } else {
            reads = 1UL << pipes[0];
            CHECK(sc(23,pipes[0]+1,(long)&reads,0,0,0,0) == 1 && reads == (1UL << pipes[0]));
        }
        char byte;
        CHECK(s3(0,pipes[0],&byte,1) == 1 && byte == 'x');
        int status = -1;
        CHECK(sc(61,child,(long)&status,0,0,0,0) == child && status == 0);
        p.events = 0;
        CHECK(s3(7,&p,1,0) == 1 && p.revents == 16);
        CHECK(s3(3,pipes[0],0,0) == 0);
    }
    struct timespec realtime, monotonic;
    CHECK(s3(228,0,&realtime,0) == 0 && realtime.sec > 1700000000);
    CHECK(s3(228,1,&monotonic,0) == 0 && monotonic.sec < realtime.sec);
    CHECK(s3(228,2,&ts,0) == -22);
    return 0;
}
__asm__(".global _start\n_start:\nxor %ebp,%ebp\nand $-16,%rsp\ncall test_main\nmov %rax,%rdi\nmov $60,%eax\nsyscall\nud2\n");
