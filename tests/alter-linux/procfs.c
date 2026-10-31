/* Process procfs ABI: numeric directories, openat, short reads, fork and reap. */
typedef unsigned long word;
extern long linux_call(long, word, word, word, word, word, word);
static long call(long n, word a, word b, word c, word d) {
    return linux_call(n, a, b, c, d, 0, 0);
}
static word decimal(char *out, word value) {
    char digits[20];
    word used = 0, count = 0;
    do { digits[count++] = '0' + value % 10; value /= 10; } while (value != 0);
    while (count != 0) { out[used++] = digits[--count]; }
    out[used] = 0;
    return used;
}
static int equal(const char *a, const char *b, word count) {
    for (word i = 0; i < count; ++i) { if (a[i] != b[i]) { return 0; } }
    return 1;
}
static int read_all(long fd, char *out, word limit) {
    word used = 0;
    while (used < limit) {
        long count = call(0, fd, (word)(out + used), 1, 0);
        if (count < 0) { return -1; }
        if (count == 0) { break; }
        used += count;
    }
    return (int)used;
}
int test_main(void) {
    char pid[21];
    word me = call(39, 0, 0, 0, 0);
    word pidlen = decimal(pid, me);
    long root = call(2, (word)"/proc", 0200000, 0, 0);
    if (root < 0) { return 10; }
    long dir = call(257, root, (word)pid, 0200000, 0);
    if (dir < 0) { return 11; }
    char metadata[144];
    if (call(5, dir, (word)metadata, 0, 0) != 0
        || ((*(unsigned int *)(metadata + 24)) & 0170000) != 0040000) { return 12; }
    long fd = call(257, dir, (word)"stat", 0, 0);
    if (fd < 0) { return 13; }
    char stat[2048];
    long bytes = call(0, fd, (word)stat, sizeof(stat), 0);
    if (bytes <= 0 || !equal(stat, pid, pidlen) || stat[pidlen] != ' ') { return 14; }
    if (call(8, fd, 0, 0, 0) != 0) { return 15; }
    char small[2048];
    if (read_all(fd, small, sizeof(small)) != bytes || !equal(stat, small, bytes)) { return 16; }
    call(3, fd, 0, 0, 0);
    fd = call(257, dir, (word)"cmdline", 0, 0);
    bytes = call(0, fd, (word)stat, sizeof(stat), 0);
    if (bytes < 1 || stat[bytes - 1] != 0) { return 17; }
    call(3, fd, 0, 0, 0);
    long tasks = call(257, dir, (word)"task", 0200000, 0);
    if (tasks < 0 || call(217, tasks, (word)stat, sizeof(stat), 0) <= 0) { return 27; }
    if (!equal(stat + 19, pid, pidlen) || stat[19 + pidlen] != 0) { return 28; }
    long thread = call(257, tasks, (word)pid, 0200000, 0);
    if (thread < 0) { return 29; }
    fd = call(257, thread, (word)"stat", 0, 0);
    bytes = call(0, fd, (word)stat, sizeof(stat), 0);
    if (bytes < 1 || !equal(stat, pid, pidlen) || stat[pidlen] != ' ') { return 30; }
    call(3, fd, 0, 0, 0); call(3, thread, 0, 0, 0); call(3, tasks, 0, 0, 0);
    /* A self-directory FD names its original process even in a fork child. */
    long self = call(2, (word)"/proc/self", 0200000, 0, 0);
    if (self < 0) { return 18; }
    long child = call(57, 0, 0, 0, 0);
    if (child < 0) { return 19; }
    if (child == 0) {
        long entry = call(257, self, (word)"stat", 0, 0);
        char buffer[2048];
        long size = call(0, entry, (word)buffer, sizeof(buffer), 0);
        if (size <= 0 || !equal(buffer, pid, pidlen) || buffer[pidlen] != ' ') { return 20; }
        return 0;
    }
    int status = -1;
    if (call(61, child, (word)&status, 0, 0) != child || status != 0) { return 21; }
    decimal(pid, child);
    if (call(257, root, (word)pid, 0200000, 0) != -2) { return 22; }
    /* Tiny buffers force getdents to resume using the returned directory cookie. */
    int found = 0;
    while ((bytes = call(217, root, (word)stat, 32, 0)) > 0) {
        for (word at = 0; at < (word)bytes;) {
            unsigned short length = *(unsigned short *)(stat + at + 16);
            if (length < 24 || at + length > (word)bytes) { return 23; }
            char expected[21];
            word len = decimal(expected, me);
            if (equal(stat + at + 19, expected, len) && stat[at + 19 + len] == 0) { found = 1; }
            at += length;
        }
    }
    if (bytes != 0 || !found || call(8, root, 0, 0, 0) != 0) { return 24; }
    if (call(217, root, (word)stat, 1, 0) != -22) { return 25; }
    if (call(217, root, (word)stat, 32, 0) <= 0) { return 26; }
    call(3, root, 0, 0, 0); call(3, dir, 0, 0, 0); call(3, self, 0, 0, 0);
    static const char pass[] = "PROCFS-PASS\n";
    call(1, 1, (word)pass, sizeof(pass) - 1, 0);
    return 0;
}
