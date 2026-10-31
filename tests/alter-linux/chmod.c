/* Persistent chmod, fchmod and *at regression, independent of libc and FPU. */
typedef unsigned long word;
extern long linux_call(long, word, word, word, word, word, word);
static long call(long n, word a, word b, word c, word d) {
    return linux_call(n, a, b, c, d, 0, 0);
}
static unsigned int mode(long fd) {
    unsigned long stat[18];
    if (call(5, fd, (word)stat, 0, 0) != 0) { return ~0u; }
    return *(unsigned int *)((char *)stat + 24);
}
int test_main(void) {
#ifdef CHMOD_READONLY
    long file = call(2, (word)"/etc/os-release", 0, 0, 0);
    if (file < 0) { return 70; }
    unsigned int before = mode(file);
    if (call(90, (word)"/etc/os-release", 0600, 0, 0) != -30) { return 71; }
    if (call(91, file, 0600, 0, 0) != -30) { return 72; }
    if (call(452, file, (word)"", 0600, 0x1000) != -30 || mode(file) != before) { return 73; }
    call(3, file, 0, 0, 0);
    static const char pass[] = "CHMOD-READONLY-PASS\n";
    call(1, 1, (word)pass, sizeof(pass) - 1, 0);
    return 0;
#else
    const char *base = "/tmp/chmod-suite";
    const char *path = "/tmp/chmod-suite/file";
    if (call(83, (word)base, 0755, 0, 0) != 0) { return 10; }
    long directory = call(2, (word)base, 65536, 0, 0);
    long file = call(2, (word)path, 0102, 0644, 0);
    long other = call(2, (word)path, 0, 0, 0);
    if (directory < 0 || file < 0 || other < 0) { return 11; }
    call(95, 077, 0, 0, 0);
    if (call(90, (word)path, 06754, 0, 0) != 0 || (mode(file) & 07777) != 06754 || (mode(other) & 07777) != 06754) { return 12; }
    if (call(91, other, 0, 0, 0) != 0 || (mode(file) & 07777) != 0) { return 13; }
    if (call(90, (word)path, 0160751, 0, 0) != 0 || mode(file) != (0100000 | 0751)) { return 14; }
    if (call(91, directory, 01770, 0, 0) != 0 || mode(directory) != (0040000 | 01770)) { return 15; }
    if (call(88, (word)"file", (word)"/tmp/chmod-suite/link", 0, 0) != 0) { return 16; }
    if (call(90, (word)"/tmp/chmod-suite/link", 0601, 0, 0) != 0 || (mode(file) & 07777) != 0601) { return 17; }
    if (call(452, directory, (word)"link", 0777, 0x100) != -95 || (mode(file) & 07777) != 0601) { return 18; }
    if (call(268, directory, (word)"file", 0620, 0xdead) != 0 || (mode(file) & 07777) != 0620) { return 19; }
    if (call(452, file, (word)"", 0640, 0x1000) != 0 || (mode(other) & 07777) != 0640) { return 20; }
    if (call(452, directory, (word)"file", 0600, 0x4000) != -22) { return 21; }
    if (call(91, ~0ul, 0600, 0, 0) != -9 || call(90, 1, 0600, 0, 0) != -14) { return 22; }
    if (call(90, (word)"/tmp/chmod-suite/missing", 0600, 0, 0) != -2) { return 23; }
    if (call(268, 999, (word)"file", 0600, 0) != -9 || call(268, file, (word)"file", 0600, 0) != -20) { return 24; }
    if (call(452, file, (word)"", 0600, 0) != -2) { return 25; }
    /* dirfd is Linux int, even on x86-64. glibc zero-extends AT_FDCWD. */
    if (call(80, (word)base, 0, 0, 0) != 0) { return 34; }
    long relative = call(257, 0xffffff9cul, (word)".", 0x90800, 0);
    if (relative < 0 || mode(relative) != mode(directory)) { return 35; }
    unsigned long relative_stat[18];
    if (call(262, 0xffffff9cul, (word)".", (word)relative_stat, 0) != 0) { return 36; }
    if (call(268, 0xffffff9cul, (word)"file", 0604, 0) != 0 || (mode(file) & 07777) != 0604) { return 37; }
    if (call(452, 0xffffff9cul, (word)"", 01771, 0x1000) != 0 || (mode(directory) & 07777) != 01771) { return 38; }
    if (call(257, 0xfffffffful, (word)"file", 0, 0) != -9) { return 39; }
    call(3, relative, 0, 0, 0);
    if (call(80, (word)"/", 0, 0, 0) != 0) { return 40; }
    if (call(82, (word)base, (word)"/tmp/chmod-moved", 0, 0) != 0) { return 26; }
    if (call(268, directory, (word)"file", 0660, 0) != 0 || (mode(other) & 07777) != 0660) { return 27; }
    if (call(86, (word)"/tmp/chmod-moved/file", (word)"/tmp/chmod-moved/hard", 0, 0) != 0) { return 28; }
    if (call(90, (word)"/tmp/chmod-moved/hard", 0600, 0, 0) != 0 || (mode(file) & 07777) != 0600) { return 29; }
    if (call(87, (word)"/tmp/chmod-moved/file", 0, 0, 0) != 0 || call(87, (word)"/tmp/chmod-moved/hard", 0, 0, 0) != 0) { return 30; }
    long replacement = call(2, (word)"/tmp/chmod-moved/replacement", 0102, 0644, 0);
    if (replacement < 0 || call(91, replacement, 0644, 0, 0) != 0) { return 31; }
    if (call(91, file, 0610, 0, 0) != 0 || (mode(other) & 07777) != 0610 || (mode(replacement) & 07777) != 0644) { return 33; }
    if (call(74, file, 0, 0, 0) != 0) { return 32; }
    call(3, file, 0, 0, 0);
    call(3, other, 0, 0, 0);
    call(3, directory, 0, 0, 0);
    call(3, replacement, 0, 0, 0);
    call(95, 022, 0, 0, 0);
    static const char pass[] = "CHMOD-PASS\n";
    call(1, 1, (word)pass, sizeof(pass) - 1, 0);
    return 0;
#endif
}
