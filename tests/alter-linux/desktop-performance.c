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

struct Iovec { word base, length; };
struct Message {
    word name;
    unsigned int name_length, padding;
    word iov, iov_count, control, control_length;
    unsigned int flags, tail;
};
_Static_assert(sizeof(struct Message) == 56, "Linux LP64 msghdr");

static int recvmsg_faults(int writer, int reader) {
    long allocation = linux_call(9, 0, 8192, 3, 0x22, (word)-1, 0);
    if (allocation < 0) { return 20; }
    word base = (word)allocation;
    word sent = 0x12345678UL, received = 0;
    struct Iovec iov = { (word)&received, sizeof(received) };
    const word boundaries[] = {44, 48};
    for (word test = 0; test < 2; ++test) {
        struct Message message = { 0, 99, 0x11223344U, (word)&iov, 1, 0, 24,
                                   0x55667788U, 0x99aabbccU };
        unsigned char *wire = (unsigned char *)(base + 4096 - boundaries[test]);
        /* Byte copies permit deliberately unaligned Linux ABI headers. */
        for (word i = 0; i < sizeof(message); ++i) {
            wire[i] = ((unsigned char *)&message)[i];
        }
        if (call(1, writer, (word)&sent, sizeof(sent), 0) != sizeof(sent)) { return 21; }
        if (call(10, base + 4096, 4096, 1, 0) != 0) { return 22; }
        if (call(47, reader, (word)wire, 0, 0) != -14) { return 23; }
        for (word i = 0; i < sizeof(message); ++i) {
            ((unsigned char *)&message)[i] = wire[i];
        }
        word expected_control = test == 0 ? 24 : 0;
        if (message.name_length != 0 || message.control_length != expected_control
            || message.flags != 0x55667788U || message.padding != 0x11223344U
            || message.tail != 0x99aabbccU || received != sent) { return 24; }
        if (call(10, base + 4096, 4096, 3, 0) != 0) { return 25; }
        /* EFAULT must leave the packet queued, including after partial metadata stores. */
        received = 0;
        if (call(47, reader, (word)wire, 0, 0) != sizeof(sent) || received != sent) { return 26; }
    }
    if (call(11, base, 8192, 0, 0) != 0) { return 27; }
    return 0;
}

static int recvmsg_performance(int writer, int reader) {
    int checked = recvmsg_faults(writer, reader);
    if (checked != 0) { return checked; }
    word received = 0;
    struct Iovec iov = { (word)&received, sizeof(received) };
    struct Message message = { 0, 0, 0x11223344U, (word)&iov, 1, 0, 0, 0, 0x99aabbccU };
    for (int sample = 0; sample < 3; ++sample) {
        word begin = now();
        for (word i = 0; i < 2048; ++i) {
            if (call(1, writer, (word)&i, sizeof(i), 0) != sizeof(i)) { return 28; }
            message.name_length = 99;
            message.flags = 0x55667788U;
            if (call(47, reader, (word)&message, 0, 0) != sizeof(i) || received != i
                || message.name_length != 0 || message.control_length != 0 || message.flags != 0
                || message.padding != 0x11223344U || message.tail != 0x99aabbccU
                || message.iov != (word)&iov || message.iov_count != 1) { return 29; }
        }
        report("DESKTOP-PERF unix-recvmsg-2048-ns=", now() - begin);
    }
    if (call(3, writer, 0, 0, 0) != 0) { return 30; }
    for (int sample = 0; sample < 3; ++sample) {
        word begin = now();
        for (word i = 0; i < 2048; ++i) {
            message.name_length = 99;
            message.control_length = 24;
            message.flags = 0x55667788U;
            if (call(47, reader, (word)&message, 0, 0) != 0
                || message.name_length != 0 || message.control_length != 0 || message.flags != 0
                || message.padding != 0x11223344U || message.tail != 0x99aabbccU) { return 31; }
        }
        report("DESKTOP-PERF unix-recvmsg-eof-2048-ns=", now() - begin);
    }
    return 0;
}

struct DirectoryResult { word entries, checksum, calls; };
static unsigned char directory_buffer[32768];

static int directory_cache_freshness(void) {
    static const char directory[] = "/tmp/nanami-directory-cache";
    static const char entry[] = "/tmp/nanami-directory-cache/entry";
    static const char moved[] = "/tmp/nanami-directory-cache/moved";
    unsigned char metadata[144];
    if (call(83, (word)directory, 0700, 0, 0) != 0) { return 51; }
    long fd = call(257, (word)-100, (word)directory, 0x10000, 0);
    long original = call(257, (word)-100, (word)entry, 0100 | 2, 0600);
    if (fd < 0 || original < 0) { return 52; }
    if (call(217, fd, (word)directory_buffer, sizeof(directory_buffer), 0) <= 0) { return 53; }
    // Keep the old inode pinned while replacing its cached pathname/type.
    if (call(87, (word)entry, 0, 0, 0) != 0) { return 54; }
    if (call(6, (word)entry, (word)metadata, 0, 0) != -2) { return 55; }
    if (call(83, (word)entry, 0700, 0, 0) != 0) { return 56; }
    if (call(6, (word)entry, (word)metadata, 0, 0) != 0
        || (((word)metadata[24] | (word)metadata[25] << 8) & 0170000) != 0040000) { return 57; }
    if (call(8, fd, 0, 0, 0) != 0
        || call(217, fd, (word)directory_buffer, sizeof(directory_buffer), 0) <= 0) { return 58; }
    if (call(82, (word)entry, (word)moved, 0, 0) != 0) { return 59; }
    if (call(6, (word)entry, (word)metadata, 0, 0) != -2
        || call(6, (word)moved, (word)metadata, 0, 0) != 0) { return 60; }
    if (call(8, fd, 0, 0, 0) != 0
        || call(217, fd, (word)directory_buffer, sizeof(directory_buffer), 0) <= 0) { return 61; }
    if (call(82, (word)moved, (word)entry, 0, 0) != 0) { return 62; }
    if (call(6, (word)moved, (word)metadata, 0, 0) != -2
        || call(6, (word)entry, (word)metadata, 0, 0) != 0
        || (((word)metadata[24] | (word)metadata[25] << 8) & 0170000) != 0040000) { return 63; }
    if (call(84, (word)entry, 0, 0, 0) != 0
        || call(3, original, 0, 0, 0) != 0
        || call(3, fd, 0, 0, 0) != 0
        || call(84, (word)directory, 0, 0, 0) != 0) { return 64; }
    static const char pass[] = "DIRECTORY-CACHE-FRESHNESS-PASS\n";
    call(1, 1, (word)pass, sizeof(pass) - 1, 0);
    return 0;
}

static int scan_directory(word capacity, struct DirectoryResult *result) {
    static const char path[] = "/usr/local/lib";
    long fd = call(257, (word)-100, (word)path, 0x10000, 0);
    if (fd < 0) { return 40; }
    result->entries = 0;
    result->checksum = 14695981039346656037UL;
    result->calls = 0;
    word previous_cookie = 0;
    int error = 0;
    for (;;) {
        long bytes = call(217, fd, (word)directory_buffer, capacity, 0);
        ++result->calls;
        if (bytes < 0 || (word)bytes > capacity) { error = 41; break; }
        if (bytes == 0) { break; }
        word position = 0;
        while (position < (word)bytes) {
            unsigned char *entry = directory_buffer + position;
            word remaining = (word)bytes - position;
            if (remaining < 24) { error = 42; break; }
            word length = (word)entry[16] | (word)entry[17] << 8;
            if (length < 24 || length % 8 != 0 || length > remaining) { error = 43; break; }
            word cookie = 0;
            for (word byte = 0; byte < 8; ++byte) { cookie |= (word)entry[8 + byte] << (byte * 8); }
            if (cookie <= previous_cookie || entry[19] == 0) { error = 44; break; }
            previous_cookie = cookie;
            word end = 19;
            while (end < length && entry[end] != 0) {
                result->checksum = (result->checksum ^ entry[end]) * 1099511628211UL;
                ++end;
            }
            if (end == length) { error = 45; break; }
            // Include record boundaries, not only concatenated name bytes.
            result->checksum = (result->checksum ^ 0xffUL) * 1099511628211UL;
            ++result->entries;
            position += length;
        }
        if (error != 0) { break; }
    }
    if (call(3, fd, 0, 0, 0) != 0 && error == 0) { error = 46; }
    return error;
}

static int busybox_ls(void) {
    long child = call(57, 0, 0, 0, 0);
    if (child < 0) { return 47; }
    if (child == 0) {
        static const char null_path[] = "/dev/null";
        long fd = call(257, (word)-100, (word)null_path, 1, 0);
        if (fd < 0 || call(33, fd, 1, 0, 0) != 1 || call(33, fd, 2, 0, 0) != 2) {
            call(231, 96, 0, 0, 0);
            for (;;) {}
        }
        if (fd > 2) { call(3, fd, 0, 0, 0); }
        static const char path[] = "/bin/busybox";
        const char *argv[] = { "busybox", "ls", "-1", "/usr/local/lib", 0 };
        const char *envp[] = { "LC_ALL=C", "LANG=C", 0 };
        call(59, (word)path, (word)argv, (word)envp, 0);
        call(231, 97, 0, 0, 0);
        for (;;) {}
    }
    word status = 0;
    if (call(61, child, (word)&status, 0, 0) != child || status != 0) { return 48; }
    return 0;
}

static int directory_performance(void) {
    struct DirectoryResult expected;
    int error = scan_directory(sizeof(directory_buffer), &expected);
    if (error != 0) { return error; }
    if (expected.entries < 32) { return 49; }
    report("DIRECTORY-CHECK entries=", expected.entries);
    report("DIRECTORY-CHECK checksum=", expected.checksum);
    const word capacities[] = { 512, sizeof(directory_buffer) };
    for (word mode = 0; mode < 2; ++mode) {
        for (word sample = 0; sample < 3; ++sample) {
            word calls = 0;
            word begin = now();
            for (word iteration = 0; iteration < 64; ++iteration) {
                struct DirectoryResult result;
                error = scan_directory(capacities[mode], &result);
                if (error != 0) { return error; }
                if (result.entries != expected.entries || result.checksum != expected.checksum) { return 50; }
                calls += result.calls;
            }
            word elapsed = now() - begin;
            report(mode == 0 ? "DIRECTORY-WORK small-calls=" : "DIRECTORY-WORK large-calls=", calls);
            report(mode == 0 ? "DESKTOP-PERF directory-small-64-ns=" : "DESKTOP-PERF directory-large-64-ns=", elapsed);
        }
    }
    for (word sample = 0; sample < 3; ++sample) {
        word begin = now();
        for (word iteration = 0; iteration < 8; ++iteration) {
            error = busybox_ls();
            if (error != 0) { return error; }
        }
        report("DESKTOP-PERF busybox-ls-8-ns=", now() - begin);
    }
    return 0;
}

int test_main(void) {
    int freshness = directory_cache_freshness();
    if (freshness != 0) { return freshness; }
    int directory = directory_performance();
    if (directory != 0) { return directory; }
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
    int messages = recvmsg_performance(sockets[0], sockets[1]);
    if (messages != 0) { return messages; }
    static const char pass[] = "DESKTOP-PERFORMANCE-PASS\n";
    call(1, 1, (word)pass, sizeof(pass) - 1, 0);
    return 0;
}
