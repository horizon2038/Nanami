/* Fixed-work, integer-only microbenchmark. Clock queries are outside the loop. */
typedef unsigned long word;
extern long linux_call(long, word, word, word, word, word, word);
static long call(long n, word a, word b, word c, word d) {
    return linux_call(n, a, b, c, d, 0, 0);
}
static word now(void) {
    word time[2];
    if (call(228, 1, (word)time, 0, 0) != 0) { return 0; }
    return time[0] * 1000000000UL + time[1];
}
static void report(const char *label, word elapsed) {
    char output[128];
    word used = 0;
    while (*label != 0) { output[used++] = *label++; }
    char digits[24];
    word count = 0;
    do { digits[count++] = '0' + elapsed % 10; elapsed /= 10; } while (elapsed != 0);
    while (count != 0) { output[used++] = digits[--count]; }
    output[used++] = '\n';
    call(1, 1, (word)output, used, 0);
}
int test_main(void) {
    for (int missing = 0; missing < 2; ++missing) {
        const char *path = missing ? "/usr/local/lib/nanami-missing-file" : "/usr/local/lib/libX11.so.6";
        for (int sample = 0; sample < 3; ++sample) {
            unsigned char stat[144];
            word begin = now();
            for (word i = 0; i < 512; ++i) {
                long result = call(262, (word)-100, (word)path, (word)stat, 0);
                if (result != (missing ? -2 : 0)) { return 13; }
            }
            report(missing ? "DESKTOP-PERF missing-stat-512-ns=" : "DESKTOP-PERF stat-512-ns=", now() - begin);
        }
    }
    int sockets[2];
    if (call(53, 1, 1, 0, (word)sockets) != 0) { return 10; }
    for (int sample = 0; sample < 3; ++sample) {
        word value = 0;
        word begin = now();
        for (word i = 0; i < 2048; ++i) {
            if (call(1, sockets[0], (word)&i, sizeof(i), 0) != sizeof(i)) { return 11; }
            if (call(0, sockets[1], (word)&value, sizeof(value), 0) != sizeof(value) || value != i) { return 12; }
        }
        report("DESKTOP-PERF unix-2048-ns=", now() - begin);
    }
    static const char pass[] = "DESKTOP-PERFORMANCE-PASS\n";
    call(1, 1, (word)pass, sizeof(pass) - 1, 0);
    return 0;
}
