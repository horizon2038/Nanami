/* Same-size data-only writes, hardlinks, path replacement and truncation must
   not execute stale cached code. No libc, dynamic linker or FPU is needed. */
typedef unsigned long word;
extern long linux_call(long, word, word, word, word, word, word);
static long call(long n, word a, word b, word c, word d) {
    return linux_call(n, a, b, c, d, 0, 0);
}
static int exec_status(const char *path, int expected) {
    long child = call(57, 0, 0, 0, 0);
    if (child < 0) { return 1; }
    if (child == 0) {
        const char *args[] = { path, 0 };
        const char *env[] = { 0 };
        long result = call(59, (word)path, (word)args, (word)env, 0);
        call(60, result == -8 ? 200 : 201, 0, 0, 0);
        return 1;
    }
    int status = -1;
    return call(61, child, (word)&status, 0, 0) != child || status != expected << 8;
}
static void put(unsigned char *p, word value, int bytes) {
    for (int i = 0; i < bytes; ++i) { p[i] = value >> (8 * i); }
}
int test_main(void) {
    const char *path = "/tmp/nanami-exec-cache";
    const char *alias = "/tmp/nanami-exec-cache-alias";
    const char *old = "/tmp/nanami-exec-cache-old";
    static unsigned char image[132];
    image[0] = 127; image[1] = 'E'; image[2] = 'L'; image[3] = 'F';
    image[4] = 2; image[5] = 1; image[6] = 1;
    put(image + 16, 2, 2); put(image + 18, 62, 2); put(image + 20, 1, 4);
    put(image + 24, 0x400078, 8); put(image + 32, 64, 8);
    put(image + 52, 64, 2); put(image + 54, 56, 2); put(image + 56, 1, 2);
    put(image + 64, 1, 4); put(image + 68, 5, 4);
    put(image + 80, 0x400000, 8); put(image + 96, sizeof(image), 8);
    put(image + 104, 4096, 8); put(image + 112, 4096, 8);
    image[120] = 0xb8; image[121] = 60; image[125] = 0xbf;
    image[126] = 7; image[130] = 0x0f; image[131] = 0x05;
    long fd = call(2, (word)path, 2 | 64 | 512, 0700, 0);
    if (fd < 0 || call(1, fd, (word)image, sizeof(image), 0) != sizeof(image)) { return 20; }
    if (exec_status(path, 7)) { return 21; }
    image[126] = 17;
    if (call(18, fd, (word)&image[126], 1, 126) != 1 || exec_status(path, 17)) { return 22; }
    if (call(86, (word)path, (word)alias, 0, 0) != 0 || exec_status(alias, 17)) { return 23; }
    if (call(82, (word)path, (word)old, 0, 0) != 0) { return 24; }
    long replacement = call(2, (word)path, 2 | 64 | 512, 0700, 0);
    image[126] = 31;
    if (replacement < 0 || call(1, replacement, (word)image, sizeof(image), 0) != sizeof(image)) { return 25; }
    if (exec_status(path, 31) || exec_status(alias, 17)) { return 26; }
    image[0] = 0;
    if (call(18, replacement, (word)image, 1, 0) != 1 || exec_status(path, 200)) { return 27; }
    if (call(77, fd, 0, 0, 0) != 0 || exec_status(alias, 200)) { return 28; }
    call(3, fd, 0, 0, 0); call(3, replacement, 0, 0, 0);
    call(87, (word)path, 0, 0, 0); call(87, (word)alias, 0, 0, 0); call(87, (word)old, 0, 0, 0);
    static const char pass[] = "PROCESS-CACHE-FRESHNESS-PASS\n";
    call(1, 1, (word)pass, sizeof(pass) - 1, 0);
    return 0;
}
