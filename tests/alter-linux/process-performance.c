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
static int syscall_controls(void) {
    for (int credentials = 0; credentials < 2; ++credentials) {
        for (int sample = 0; sample < 3; ++sample) {
            word begin = now();
            for (int i = 0; i < 2048; ++i) {
                long value = call(credentials ? 102 : 39, 0, 0, 0, 0);
                if (credentials) {
                    if (value != 0) { return 33; }
                } else if (value <= 0) { return 34; }
            }
            report(credentials ? "PROCESS-PERF getuid-2048-ns=" : "PROCESS-PERF getpid-2048-ns=", now() - begin);
        }
    }
    return 0;
}
static int copyout_performance(void) {
    const word vma_counts[] = {0, 128, 256};
    for (word mode = 0; mode < sizeof(vma_counts) / sizeof(vma_counts[0]); ++mode) {
        word count = vma_counts[mode];
        for (word i = 0; i < count; ++i) {
            word address = 0x500000000UL + i * 8192;
            if (linux_call(9, address, 4096, 0, 2 | 0x10 | 0x20, -1, 0) != (long)address) { return 30; }
        }
        const char *label;
        if (count == 0) { label = "PROCESS-PERF copyout-normal-ns="; }
        else if (count == 128) { label = "PROCESS-PERF copyout-128-vmas-ns="; }
        else { label = "PROCESS-PERF copyout-256-vmas-ns="; }
        for (int sample = 0; sample < 3; ++sample) {
            word start = now();
            for (int i = 0; i < 2048; ++i) {
                unsigned int ids[3] = { 99, 99, 99 };
                if (call(118, (word)&ids[0], (word)&ids[1], (word)&ids[2], 0) != 0
                    || ids[0] != 0 || ids[1] != 0 || ids[2] != 0) { return 31; }
            }
            report(label, now() - start);
        }
        for (word i = 0; i < count; ++i) {
            if (call(11, 0x500000000UL + i * 8192, 4096, 0, 0) != 0) { return 32; }
        }
    }
    return 0;
}
static int fragmented_fork_performance(void) {
    const word counts[] = {128, 256};
    for (word mode = 0; mode < sizeof(counts) / sizeof(counts[0]); ++mode) {
        word count = counts[mode];
        for (word i = 0; i < count; ++i) {
            word address = 0x500000000UL + i * 8192;
            if (linux_call(9, address, 4096, 0, 2 | 0x10 | 0x20, -1, 0) != (long)address) { return 40; }
        }
        for (int sample = 0; sample < 3; ++sample) {
            word begin = now();
            for (int iteration = 0; iteration < 8; ++iteration) {
                long child = call(57, 0, 0, 0, 0);
                if (child < 0) { return 41; }
                if (child == 0) {
                    /* Unbacked PROT_NONE reservations must survive fork. */
                    unsigned int effective = 99, saved = 99;
                    long result = call(118, 0x500000000UL, (word)&effective, (word)&saved, 0);
                    call(60, result == -14 ? 0 : 43, 0, 0, 0);
                    return 44;
                }
                int status = -1;
                if (call(61, child, (word)&status, 0, 0) != child || status != 0) { return 42; }
            }
            report(count == 128 ? "PROCESS-PERF fork-128-vmas-8-ns=" : "PROCESS-PERF fork-256-vmas-8-ns=", now() - begin);
        }
        for (word i = 0; i < count; ++i) {
            if (call(11, 0x500000000UL + i * 8192, 4096, 0, 0) != 0) { return 45; }
        }
    }
    return 0;
}
static int private_heap_fork_performance(void) {
    const word pages = 256, bytes = pages * 4096;
    long base = linux_call(9, 0, bytes, 3, 0x22, -1UL, 0);
    if (base < 0) { return 50; }
    volatile unsigned char *p = (void *)base;
    for (word i = 0; i < pages; ++i) { p[i * 4096] = (unsigned char)i; }
    for (word sample = 0; sample < 3; ++sample) {
        word begin = now();
        for (word i = 0; i < 8; ++i) {
            long child = call(57, 0, 0, 0, 0);
            if (child < 0) { return 51; }
            if (child == 0) { call(60, 0, 0, 0, 0); return 52; }
            int status = -1;
            if (call(61, child, (word)&status, 0, 0) != child || status != 0) { return 53; }
        }
        report("PROCESS-PERF fork-private-1m-8-ns=", now() - begin);
        for (word i = 0; i < pages; ++i) {
            if (p[i * 4096] != (unsigned char)i) { return 54; }
        }
    }
    if (call(11, base, bytes, 0, 0) != 0) { return 55; }
    return 0;
}

int test_main(void) {
    /* Isolate syscall/copyout work before fork/exec changes the process tree. */
    int controls = syscall_controls();
    if (controls != 0) { return controls; }
    int copied = copyout_performance();
    if (copied != 0) { return copied; }
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
    int fragmented = fragmented_fork_performance();
    if (fragmented != 0) { return fragmented; }
    int private_heap = private_heap_fork_performance();
    if (private_heap != 0) { return private_heap; }
    static const char pass[] = "PROCESS-PERFORMANCE-PASS\n";
    call(1, 1, (word)pass, sizeof(pass) - 1, 0);
    return 0;
}
