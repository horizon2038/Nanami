/* Rooted symlinks and inode-based *at regressions; no libc or FPU required. */
typedef unsigned long word;
extern long linux_call(long, word, word, word, word, word, word);
static long call(long n, word a, word b, word c, word d) { return linux_call(n, a, b, c, d, 0, 0); }
static int equal(const char *a, const char *b, word bytes) {
    for (word index = 0; index < bytes; index++) { if (a[index] != b[index]) { return 0; } }
    return 1;
}
static int read_value(long fd) {
    char output[4] = { 0 };
    long bytes = call(0, fd, (word)output, 4, 0);
    call(3, fd, 0, 0, 0);
    return bytes == 4 && equal(output, "data", 4);
}
int test_main(void) {
    const char *base = "/tmp/ns-suite";
    if (call(83, (word)base, 0755, 0, 0) != 0) { return 10; }
    if (call(83, (word)"/tmp/ns-suite/a", 0755, 0, 0) != 0 || call(83, (word)"/tmp/ns-suite/a/sub", 0755, 0, 0) != 0) { return 11; }
    if (call(88, (word)"a/sub", (word)"/tmp/ns-suite/link", 0, 0) != 0) { return 12; }
    long fd = call(2, (word)"/tmp/ns-suite/link/../value", 2 | 64 | 512, 0600, 0);
    if (fd < 0 || call(1, fd, (word)"data", 4, 0) != 4) { return 13; }
    call(3, fd, 0, 0, 0);
    fd = call(2, (word)"/tmp/ns-suite/a/value", 0, 0, 0);
    if (fd < 0 || !read_value(fd)) { return 14; }
    if (call(2, (word)"/tmp/ns-suite/value", 0, 0, 0) != -2) { return 15; }
    if (call(88, (word)"/tmp/ns-suite/a/value", (word)"/tmp/ns-suite/absolute", 0, 0) != 0) { return 16; }
    fd = call(2, (word)"/tmp/ns-suite/absolute", 0, 0, 0);
    if (fd < 0 || !read_value(fd)) { return 17; }
    if (call(2, (word)"/tmp/ns-suite/absolute", 0x20000, 0, 0) != -40) { return 18; }
    long directory = call(2, (word)"/tmp/ns-suite/a", 65536, 0, 0);
    if (directory < 0 || call(82, (word)"/tmp/ns-suite/a", (word)"/tmp/ns-suite/moved", 0, 0) != 0) { return 19; }
    fd = call(257, directory, (word)"value", 0, 0);
    if (fd < 0 || !read_value(fd)) { return 20; }
    if (call(258, directory, (word)"child", 0755, 0) != 0) { return 21; }
    if (call(266, (word)"value", directory, (word)"relative", 0) != 0) { return 22; }
    static char target[128];
    if (call(267, directory, (word)"relative", (word)target, sizeof(target)) != 5 || !equal(target, "value", 5)) { return 23; }
    static unsigned char metadata[144];
    if (call(262, directory, (word)"relative", (word)metadata, 0x100) != 0 || (*(unsigned int *)(metadata + 24) & 0170000) != 0120000) { return 24; }
    fd = call(257, directory, (word)"relative", 0, 0);
    if (fd < 0 || !read_value(fd)) { return 25; }
    if (call(263, directory, (word)"relative", 0, 0) != 0) { return 26; }
    if (linux_call(264, directory, (word)"value", directory, (word)"moved-value", 0, 0) != 0) { return 27; }
    if (linux_call(265, directory, (word)"moved-value", directory, (word)"hard", 0, 0) != 0) { return 28; }
    fd = call(257, directory, (word)"hard", 0, 0);
    if (fd < 0 || !read_value(fd)) { return 29; }
    char long_target[128];
    for (int index = 0; index < 70; index += 2) { long_target[index] = '.'; long_target[index + 1] = '/'; }
    for (int index = 0; index < 12; index++) { long_target[70 + index] = "moved-value"[index]; }
    if (call(266, (word)long_target, directory, (word)"long", 0) != 0) { return 30; }
    fd = call(257, directory, (word)"long", 0, 0);
    if (fd < 0 || !read_value(fd) || call(263, directory, (word)"long", 0, 0) != 0) { return 31; }
    if (call(266, (word)"loop", directory, (word)"loop", 0) != 0 || call(257, directory, (word)"loop", 0, 0) != -40) { return 32; }
    if (call(266, (word)"/nanami/system-list", directory, (word)"escape", 0) != 0 || call(257, directory, (word)"escape", 0, 0) != -2) { return 33; }
    if (call(81, directory, 0, 0, 0) != 0) { return 34; }
    static char cwd[128];
    if (call(79, (word)cwd, sizeof(cwd), 0, 0) < 0 || !equal(cwd, "/tmp/ns-suite/moved", 19)) { return 35; }
    call(3, directory, 0, 0, 0);
    static const char pass[] = "NAMESPACE-PASS\n";
    call(1, 1, (word)pass, sizeof(pass) - 1, 0);
    return 0;
}
