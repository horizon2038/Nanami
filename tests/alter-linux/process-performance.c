/* Whole fork / fork+exec latency; integer-only, no timing inside child work. */
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
static void report(const char *name, word ns) {
    char output[128], digits[24];
    word length = 0, count = 0;
    while (*name != 0) { output[length++] = *name++; }
    do { digits[count++] = '0' + ns % 10; ns /= 10; } while (ns != 0);
    while (count != 0) { output[length++] = digits[--count]; }
    output[length++] = '\n';
    call(1, 1, (word)output, length, 0);
}
static int copyout_performance(void) {
    for (int fragmented = 0; fragmented < 2; ++fragmented) {
        if (fragmented != 0) {
            for (word i = 0; i < 128; ++i) {
                word address = 0x500000000UL + i * 8192;
                if (linux_call(9, address, 4096, 0, 2 | 0x10 | 0x20, -1, 0) != (long)address) { return 30; }
            }
        }
        for (int sample = 0; sample < 3; ++sample) {
            word start = now();
            for (int i = 0; i < 2048; ++i) {
                unsigned int ids[3] = { 99, 99, 99 };
                if (call(118, (word)&ids[0], (word)&ids[1], (word)&ids[2], 0) != 0
                    || ids[0] != 0 || ids[1] != 0 || ids[2] != 0) { return 31; }
            }
            report(fragmented ? "PROCESS-PERF copyout-128-vmas-ns=" : "PROCESS-PERF copyout-normal-ns=", now() - start);
        }
    }
    for (word i = 0; i < 128; ++i) {
        if (call(11, 0x500000000UL + i * 8192, 4096, 0, 0) != 0) { return 32; }
    }
    return 0;
}
int test_main(void) {
    for (int exec = 0; exec < 2; ++exec) {
        for (int sample = 0; sample < 3; ++sample) {
            word begin = now();
            for (int iteration = 0; iteration < 8; ++iteration) {
                long child = call(57, 0, 0, 0, 0);
                if (child < 0) { return 10; }
                if (child == 0) {
                    if (exec != 0) {
                        const char *args[] = { "/bin/busybox", "true", 0 };
                        const char *env[] = { "PATH=/bin:/usr/bin", 0 };
                        call(59, (word)args[0], (word)args, (word)env, 0);
                        return 11;
                    }
                    return 0;
                }
                int status = -1;
                if (call(61, child, (word)&status, 0, 0) != child || status != 0) { return 12; }
            }
            report(exec ? "PROCESS-PERF fork-exec-8-ns=" : "PROCESS-PERF fork-8-ns=", now() - begin);
        }
    }
    int copied = copyout_performance();
    if (copied != 0) { return copied; }
    static const char pass[] = "PROCESS-PERFORMANCE-PASS\n";
    call(1, 1, (word)pass, sizeof(pass) - 1, 0);
    return 0;
}
