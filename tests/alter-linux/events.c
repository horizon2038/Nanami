/* Freestanding x86-64 Linux eventfd/epoll regression. Reuse signals-x86_64.S. */
typedef unsigned long Word;
typedef unsigned long long U64;
extern long linux_call(long, long, long, long, long, long, long);
#define sc(n,a,b,c) linux_call(n, (long)(a), (long)(b), (long)(c), 0, 0, 0)
#define sc4(n,a,b,c,d) linux_call(n, (long)(a), (long)(b), (long)(c), (long)(d), 0, 0)

static void put64(unsigned char *p, U64 value)
{
    for (Word i = 0; i < 8; ++i) {
        p[i] = (unsigned char)(value >> (8 * i));
    }
}

static U64 get64(const unsigned char *p)
{
    U64 value = 0;
    for (Word i = 0; i < 8; ++i) {
        value |= (U64)p[i] << (8 * i);
    }
    return value;
}

int test_main(void)
{
    if ((sc(72, 0, 3, 0) & 3) != 2 || (sc(72, 1, 3, 0) & 3) != 2) {
        return 16;
    }
    int pipes[2];
    int nonblocking = 1;
    char byte;
    if (sc(16, 0, 0x5421, &nonblocking) != 0 || sc(0, 0, &byte, 1) != -11) {
        return 17;
    }
    nonblocking = 0;
    if (sc(16, 0, 0x5421, &nonblocking) != 0) {
        return 18;
    }
    nonblocking = 1;
    if (sc(293, pipes, 0x80000, 0) != 0) {
        return 13;
    }
    long pipe_alias = sc(32, pipes[0], 0, 0);
    if (pipe_alias < 0 || sc(16, pipes[0], 0x5421, &nonblocking) != 0
        || (sc(72, pipe_alias, 3, 0) & 0x800) == 0
        || (sc(72, pipes[1], 3, 0) & 0x800) != 0
        || (sc(72, pipes[0], 3, 0) & 3) != 0
        || (sc(72, pipes[1], 3, 0) & 3) != 1
        || sc(0, pipe_alias, &byte, 1) != -11) {
        return 14;
    }
    nonblocking = 0;
    if (sc(16, pipe_alias, 0x5421, &nonblocking) != 0
        || (sc(72, pipes[0], 3, 0) & 0x800) != 0
        || sc(16, pipe_alias, 0x5421, 0) != -14) {
        return 15;
    }
    sc(3, pipe_alias, 0, 0);
    sc(3, pipes[0], 0, 0);
    sc(3, pipes[1], 0, 0);
    long resolution[2];
    if (sc(229, 1, resolution, 0) != 0 || resolution[0] != 0 || resolution[1] <= 0
        || sc(229, 1, 0, 0) != 0 || sc(229, 99, resolution, 0) != -22) {
        return 1;
    }
    long fd = sc(290, 0, 0x80800, 0);
    long ep = sc(291, 0x80000, 0, 0);
    if (fd < 0 || ep < 0 || sc(72, fd, 1, 0) != 1 || sc(72, ep, 1, 0) != 1) {
        return 2;
    }
    U64 value = 0;
    if (sc(0, fd, &value, 8) != -11) {
        return 3;
    }
    unsigned char event[12] = { 1 };
    const U64 token = 0x1122334455667788ULL;
    put64(event + 4, token);
    if (sc4(233, ep, 1, fd, event) != 0 || sc4(233, ep, 1, fd, event) != -17
        || sc4(232, ep, event, 1, 0) != 0) {
        return 4;
    }
    value = 7;
    if (sc(1, fd, &value, 8) != 8 || sc4(232, ep, event, 1, 0) != 1
        || get64(event + 4) != token || sc4(232, ep, event, 1, 0) != 1) {
        return 5;
    }
    long alias = sc(32, fd, 0, 0);
    if (alias < 0 || sc(3, fd, 0, 0) != 0 || sc4(232, ep, event, 1, 0) != 1
        || sc(0, alias, &value, 8) != 8 || value != 7) {
        return 6;
    }
    if (sc(3, alias, 0, 0) != 0 || sc4(232, ep, event, 1, 0) != 0) {
        return 7;
    }
    fd = sc(290, 0, 0, 0);
    event[0] = 1;
    put64(event + 4, token);
    if (fd < 0 || sc4(233, ep, 1, fd, event) != 0) {
        return 8;
    }
    long child = sc(57, 0, 0, 0);
    if (child < 0) {
        return 9;
    }
    if (child == 0) {
        long delay[2] = { 0, 2000000 };
        value = 3;
        long result = sc(35, delay, 0, 0);
        if (result == 0) {
            result = sc(1, fd, &value, 8);
        }
        sc(231, result == 8 ? 0 : 10, 0, 0);
        return 10;
    }
    if (sc4(232, ep, event, 1, 1000) != 1 || get64(event + 4) != token
        || sc(0, fd, &value, 8) != 8 || value != 3) {
        return 11;
    }
    int status = -1;
    if (sc4(61, child, &status, 0, 0) != child || status != 0
        || sc(3, fd, 0, 0) != 0 || sc(3, ep, 0, 0) != 0) {
        return 12;
    }
    const char message[] = "events-ok\n";
    sc(1, 1, message, sizeof(message) - 1);
    return 0;
}
