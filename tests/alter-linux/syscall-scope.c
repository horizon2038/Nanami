/* Known syscall stream for the opt-in native scope, not a timing benchmark. */
typedef unsigned long word;
extern long linux_call(long, word, word, word, word, word, word);
static long call(long number, word a, word b, word c, word d) {
    return linux_call(number, a, b, c, d, 0, 0);
}
static long name(const char *value) {
    return call(157, 15, (word)value, 0, 0);
}
static int reaped(long child) {
    int status = -1;
    return child > 0 && call(61, (word)child, (word)&status, 0, 0) == child && status == 0;
}
static void finish_child(int status) {
    call(60, (word)status, 0, 0, 0);
    for (;;) { }
}
enum { REPORT_WORDS = 1034, REPORT_BYTES = 8272 };
static word packet[REPORT_WORDS];
static long readout(word address, word capacity) {
    /* The sixth syscall register is not part of prctl's five-argument ABI. */
    return linux_call(157, 0x4e530001, address, capacity, 0, 0, 0x5555);
}
static int store(const char *path) {
    long fd = call(2, (word)path, 01001 | 0100, 0600, 0);
    if (fd < 0) { return 0; }
    word offset = 0;
    while (offset < REPORT_BYTES) {
        long bytes = call(1, (word)fd, (word)packet + offset, REPORT_BYTES - offset, 0);
        if (bytes <= 0 || (word)bytes > REPORT_BYTES - offset) { call(3, (word)fd, 0, 0, 0); return 0; }
        offset += (word)bytes;
    }
    return call(3, (word)fd, 0, 0, 0) == 0;
}
static int checked_report(word epoch, word mode, const char *path) {
    if (readout(1, REPORT_BYTES) != -14 || readout((word)packet, 16) != -22) { return 0; }
    if (readout((word)packet, REPORT_BYTES) != REPORT_BYTES) { return 0; }
    if (packet[0] != 0x00315043534e414eUL || packet[1] != epoch || packet[2] != mode
        || packet[4] < packet[3] || packet[5] != 1 || packet[6] != 513 || packet[7] != 1) { return 0; }
    for (word number = 0; number < 513; ++number) {
        word expected = 0;
        if (number == 39) { expected = mode == 0 ? 7 : 5; }
        if (mode == 0) {
            if (number == 57 || number == 60 || number == 61 || number == 512) { expected = 1; }
            if (number == 157) { expected = 2; }
        }
        word calls = packet[8 + number * 2];
        word ticks = packet[9 + number * 2];
        if (calls != expected) { return 0; }
        if (mode == 0 || number != 39) {
            if (ticks != 0) { return 0; }
        } else if (ticks == 0 || ticks > packet[4] - packet[3]) { return 0; }
    }
    return store(path);
}
static int stopped_owner_exit(void) {
    int ready[2];
    if (call(293, (word)ready, 0, 0, 0) != 0) { return 0; }
    long child = call(57, 0, 0, 0, 0);
    if (child == 0) {
        call(3, ready[0], 0, 0, 0);
        if (name("nscope.count") != 0 || call(1, ready[1], (word)"x", 1, 0) != 1) { finish_child(1); }
        finish_child(0);
    }
    call(3, ready[1], 0, 0, 0);
    char byte;
    if (child <= 0 || call(0, ready[0], (word)&byte, 1, 0) != 1 || byte != 'x') { return 0; }
    call(3, ready[0], 0, 0, 0);
    int armed = 0;
    for (int attempt = 0; attempt < 1000; ++attempt) {
        long result = name("nscope.count");
        if (result == 0) { armed = 1; break; }
        if (result != -22) { return 0; }
        word delay[] = {0, 1000000};
        if (call(35, (word)delay, 0, 0, 0) != 0) { return 0; }
    }
    if (!armed || name("nscope.stop") != 0) { return 0; }
    return reaped(child);
}
static int stopped_owner_exec(void) {
    int ready[2], done[2];
    if (call(293, (word)ready, 0, 0, 0) != 0 || call(293, (word)done, 0, 0, 0) != 0) { return 0; }
    long child = call(57, 0, 0, 0, 0);
    if (child == 0) {
        if (call(33, ready[1], 1, 0, 0) != 1 || call(33, done[0], 0, 0, 0) != 0) { finish_child(1); }
        for (int i = 0; i < 2; ++i) {
            if (ready[i] != 0 && ready[i] != 1) { call(3, ready[i], 0, 0, 0); }
            if (done[i] != 0 && done[i] != 1) { call(3, done[i], 0, 0, 0); }
        }
        if (name("nscope.count") != 0) { finish_child(2); }
        const char *argv[] = {"busybox", "sh", "-c", "printf x; read end", 0};
        const char *env[] = {0};
        call(59, (word)"/bin/busybox", (word)argv, (word)env, 0);
        finish_child(3);
    }
    call(3, ready[1], 0, 0, 0); call(3, done[0], 0, 0, 0);
    char byte;
    if (child <= 0 || call(0, ready[0], (word)&byte, 1, 0) != 1 || byte != 'x') { return 0; }
    if (name("nscope.count") != 0 || name("nscope.stop") != 0) { return 0; }
    if (call(1, done[1], (word)"done\n", 5, 0) != 5) { return 0; }
    call(3, ready[0], 0, 0, 0); call(3, done[1], 0, 0, 0);
    return reaped(child);
}
static int exercise(int *enabled) {
    long pid = call(39, 0, 0, 0, 0);
    if (pid <= 0 || name("nscope.count") != 0) { return 10; }
    long duplicate = name("nscope.count");
    if (duplicate != 0 && duplicate != -22) { return 11; }
    *enabled = duplicate == -22;
    long child = call(57, 0, 0, 0, 0);
    if (child == 0) {
        long result = name("nscope.stop");
        long expected = 0;
        if (*enabled) { expected = -22; }
        finish_child(result == expected ? 0 : 1);
    }
    if (!reaped(child)) { return 12; }
    for (int i = 0; i < 7; ++i) {
        if (call(39, 0, 0, 0, 0) != pid) { return 13; }
    }
    if (call(999999, 0, 0, 0, 0) != -38 || name("nscope.stop") != 0) { return 14; }
    if (*enabled) {
        if (!checked_report(1, 0, "/tmp/scope-count.bin")) { return 21; }
        child = call(57, 0, 0, 0, 0);
        if (child == 0) { finish_child(readout((word)packet, REPORT_BYTES) == -1 ? 0 : 1); }
        if (!reaped(child)) { return 22; }
    } else if (readout((word)packet, REPORT_BYTES) != -22) { return 23; }
    if (name("nscope.time") != 0) { return 15; }
    for (int i = 0; i < 5; ++i) {
        if (call(39, 0, 0, 0, 0) != pid) { return 16; }
    }
    if (name("nscope.stop") != 0) { return 17; }
    if (*enabled && !checked_report(2, 1, "/tmp/scope-time.bin")) { return 24; }
    if (!stopped_owner_exit()) { return 18; }
    if (!stopped_owner_exec()) { return 19; }
    return 0;
}
int test_main(void) {
    int enabled = 0;
    int result = exercise(&enabled);
    const char *message = "SCOPE-FIXTURE-FAIL\n";
    word bytes = 19;
    if (result == 0) {
        if (enabled) {
            message = "SCOPE-FIXTURE-PASS enabled=1\n";
        } else {
            message = "SCOPE-FIXTURE-PASS enabled=0\n";
        }
        bytes = 29;
    }
    if (call(1, 1, (word)message, bytes, 0) != (long)bytes) { return 20; }
    return result;
}
