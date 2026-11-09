/* Shared aliases, private initialization and fork of a deleted executable. */
typedef unsigned long word;
extern long linux_call(long, word, word, word, word, word, word);
static long call(long n, word a, word b, word c, word d) {
    return linux_call(n, a, b, c, d, 0, 0);
}
_Alignas(4096) static volatile unsigned char protected_state[4096];
_Alignas(4096) static volatile unsigned char readlink_state[4096];
extern long thread_start(word, word, word, word, word, word);
extern const unsigned char exec_stack_probe_start[], exec_stack_probe_end[];

static void elf_word(unsigned char *p, word value, word bytes) {
    for (word i = 0; i < bytes; ++i) { p[i] = value >> (i * 8); }
}

static int exec_demand_stack_cases(void) {
    static unsigned char image[4096];
    word code_bytes = (word)(exec_stack_probe_end - exec_stack_probe_start);
    if (code_bytes > sizeof(image) - 120) { return 227; }
    word bytes = sizeof(image);
    image[0] = 127; image[1] = 'E'; image[2] = 'L'; image[3] = 'F';
    image[4] = 2; image[5] = 1; image[6] = 1;
    elf_word(image + 16, 2, 2); elf_word(image + 18, 62, 2); elf_word(image + 20, 1, 4);
    elf_word(image + 24, 0x400078, 8); elf_word(image + 32, 64, 8);
    elf_word(image + 52, 64, 2); elf_word(image + 54, 56, 2); elf_word(image + 56, 1, 2);
    elf_word(image + 64, 1, 4); elf_word(image + 68, 5, 4);
    elf_word(image + 80, 0x400000, 8); elf_word(image + 96, bytes, 8);
    elf_word(image + 104, bytes, 8); elf_word(image + 112, 4096, 8);
    for (word i = 0; i < code_bytes; ++i) { image[120 + i] = exec_stack_probe_start[i]; }
    const char *path = "/tmp/exec-demand-stack";
    long fd = call(2, (word)path, 01102, 0700, 0);
    if (fd < 0 || call(1, fd, (word)image, bytes, 0) != (long)bytes
        || call(3, fd, 0, 0, 0) != 0) { return 228; }
    volatile unsigned char *a = (void *)0x4000000;
    volatile unsigned char *b = (void *)0x4001000;
    *a = 0x6a; *b = 0x7b;
    long child = call(57, 0, 0, 0, 0);
    if (child < 0) { return 228; }
    if (child == 0) {
        const char *argv[] = {path, 0};
        const char *env[] = {0};
        call(59, (word)path, (word)argv, (word)env, 0);
        call(60, 229, 0, 0, 0);
        for (;;) {}
    }
    int status = -1;
    if (call(61, child, (word)&status, 0, 0) != child || status != 0
        || *a != 0x6a || *b != 0x7b) { return 230; }
    /* A writable private main image must not modify the immutable cached ELF. */
    unsigned char disk = 0xff;
    fd = call(2, (word)path, 0, 0, 0);
    if (fd < 0 || call(17, fd, (word)&disk, 1, 3840) != 1 || disk != 0
        || call(3, fd, 0, 0, 0) != 0) { return 230; }
    child = call(57, 0, 0, 0, 0);
    if (child < 0) { return 230; }
    if (child == 0) {
        const char *argv[] = {path, 0}; const char *env[] = {0};
        call(59, (word)path, (word)argv, (word)env, 0);
        call(60, 229, 0, 0, 0); for (;;) {}
    }
    status = -1;
    if (call(61, child, (word)&status, 0, 0) != child || status != 0
        || call(87, (word)path, 0, 0, 0) != 0) { return 230; }
    *a = 0; *b = 0;
    static const char pass[] = "EXEC-STACK-DEMAND-PASS\n";
    call(1, 1, (word)pass, sizeof(pass) - 1, 0);
    return 0;
}

static int brk_collision_cases(void) {
    long base = call(12, 0, 0, 0, 0);
    if (base <= 0 || call(12, base + 16, 0, 0, 0) != base + 16) { return 231; }
    volatile unsigned char *heap = (void *)base;
    *heap = 0xa5;
    long neighbor = linux_call(9, 0, 4096, 3, 0x22, -1ul, 0);
    if (neighbor != base + 4096) { return 232; }
    *(volatile unsigned char *)neighbor = 0x9c;
    if (call(12, base + 9 * 4096, 0, 0, 0) != base + 16
        || call(12, 0, 0, 0, 0) != base + 16) { return 233; }
    /* A rejected extension must not move Alpha's allocation cursor. */
    long probe = linux_call(9, 0, 4096, 3, 0x22, -1ul, 0);
    if (probe != neighbor + 4096 || *(volatile unsigned char *)probe != 0
        || *heap != 0xa5 || *(volatile unsigned char *)neighbor != 0x9c) { return 234; }
    if (call(11, neighbor, 4096, 0, 0) != 0 || call(11, probe, 4096, 0, 0) != 0) { return 235; }
    /* The gap is now free. Grow in place across Alpha's old automatic cursor,
       then verify that a subsequent automatic mmap starts after the break. */
    long end = base + 9 * 4096;
    if (call(12, end, 0, 0, 0) != end || *heap != 0xa5
        || *(volatile unsigned char *)(end - 1) != 0 || call(12, 0, 0, 0, 0) != end) { return 236; }
    *(volatile unsigned char *)(end - 1) = 0x6d;
    long follow = linux_call(9, 0, 4096, 3, 0x22, -1ul, 0);
    if (follow != end || *(volatile unsigned char *)(end - 1) != 0x6d
        || *heap != 0xa5 || call(11, follow, 4096, 0, 0) != 0) { return 237; }
    static const char pass[] = "BRK-COLLISION-PASS\n";
    call(1, 1, (word)pass, sizeof(pass) - 1, 0);
    return 0;
}
static volatile int demand_stack_go;
static int demand_stack_worker(void) {
    while (!__atomic_load_n(&demand_stack_go, __ATOMIC_ACQUIRE)) { call(24, 0, 0, 0, 0); }
    for (word page = 0; page < 2; ++page) {
        volatile unsigned char *address = (void *)(0x4004000 + page * 4096);
        if (*address != 0) { return 209; }
        *address = 0x8d;
    }
    while (__atomic_load_n(&demand_stack_go, __ATOMIC_ACQUIRE) != 2) { call(24, 0, 0, 0, 0); }
    return *(volatile unsigned char *)0x4006000 != 0;
}

static int shared_demand_stack_cases(void) {
    long stack = linux_call(9, 0, 65536, 3, 0x22, -1ul, 0);
    if (stack < 0) { return 209; }
    __atomic_store_n(&demand_stack_go, 0, __ATOMIC_RELEASE);
    long child = thread_start(0x100 | 17, stack + 65536, 0, 0, 0, (word)demand_stack_worker);
    if (child < 0) { return 209; }
    __atomic_store_n(&demand_stack_go, 1, __ATOMIC_RELEASE);
    for (word page = 0; page < 2; ++page) {
        volatile unsigned char *address = (void *)(0x4004000 + page * 4096);
        unsigned char value = *address;
        if (value != 0 && value != 0x8d) { return 210; }
    }
    /* Populate another untouched stack page through syscall copyout while the
       worker remains live; this must also initialize before publishing. */
    long zero = call(2, (word)"/dev/zero", 0, 0, 0);
    int copy_bad = zero < 0 || call(0, zero, 0x4006000, 4096, 0) != 4096;
    if (zero >= 0) { call(3, zero, 0, 0, 0); }
    __atomic_store_n(&demand_stack_go, 2, __ATOMIC_RELEASE);
    int status = -1;
    if (copy_bad || call(61, child, (word)&status, 0, 0) != child || status != 0) { return 210; }
    if (*(volatile unsigned char *)0x4004000 != 0x8d
        || *(volatile unsigned char *)0x4005000 != 0x8d
        || call(11, stack, 65536, 0, 0) != 0) { return 210; }
    return 0;
}

static int demand_stack_cases(void) {
    /* These bottom pages are far below this fixture's live call stack. */
    volatile unsigned char *a = (void *)0x4000000;
    volatile unsigned char *b = (void *)0x4001000;
    volatile unsigned char *c = (void *)0x4002000;
    if (*a != 0 || *b != 0) { return 201; }
    *a = 0x5a;
    long child = call(57, 0, 0, 0, 0);
    if (child < 0) { return 202; }
    if (child == 0) {
        int bad = *a != 0x5a || *c != 0;
        *a = 0x6b;
        *c = 0x7c;
        call(60, bad ? 203 : 0, 0, 0, 0);
        for (;;) {}
    }
    int status = -1;
    if (call(61, child, (word)&status, 0, 0) != child || status != 0
        || *a != 0x5a || *c != 0) { return 203; }
    /* An absent PTE with PROT_NONE must still fault, never be repopulated. */
    if (call(10, (word)b, 4096, 0, 0) != 0) { return 204; }
    child = call(57, 0, 0, 0, 0);
    if (child < 0) { return 204; }
    if (child == 0) {
        unsigned char value = *b;
        call(60, value, 0, 0, 0);
        for (;;) {}
    }
    status = -1;
    if (call(61, child, (word)&status, 0, 0) != child || (status & 127) != 11
        || call(10, (word)b, 4096, 3, 0) != 0 || *b != 0) { return 205; }
    /* An unmapped stack page must stay a hole despite Alpha's zero plan. */
    if (call(11, (word)a, 4096, 0, 0) != 0) { return 206; }
    child = call(57, 0, 0, 0, 0);
    if (child < 0) { return 206; }
    if (child == 0) {
        unsigned char value = *a;
        call(60, value, 0, 0, 0);
        for (;;) {}
    }
    status = -1;
    if (call(61, child, (word)&status, 0, 0) != child || (status & 127) != 11) { return 207; }
    if (linux_call(9, (word)a, 4096, 3, 0x32, -1ul, 0) != (long)a || *a != 0) { return 208; }
    static const char pass[] = "STACK-DEMAND-PASS\n";
    call(1, 1, (word)pass, sizeof(pass) - 1, 0);
    return 0;
}

struct directory_record {
    word inode;
    word next;
    unsigned short bytes;
    unsigned char kind;
    char name[];
} __attribute__((packed));

static int root_directory_cursor_cases(void) {
    unsigned char buffer[4096];
    char next_name[256];
    long root = call(2, (word)"/", 0200000, 0, 0);
    if (root < 0) { return 193; }
    long alias = call(32, root, 0, 0, 0);
    if (alias < 0) { return 193; }
    if (call(217, root, (word)buffer, sizeof(buffer), 0) <= 0) { return 193; }
    long prefix_cookie = call(8, alias, 0, 1, 0);
    if (prefix_cookie <= 0) { return 193; }
    long bytes = call(217, alias, (word)buffer, sizeof(buffer), 0);
    if (bytes < 48) { return 194; }
    struct directory_record *first = (void *)buffer;
    if (first->bytes < 24 || first->bytes + 24 > bytes) { return 194; }
    struct directory_record *second = (void *)(buffer + first->bytes);
    if (second->bytes < 24 || first->bytes + second->bytes > bytes) { return 194; }
    word name_bytes = 0;
    while (name_bytes < sizeof(next_name) && 19 + name_bytes < second->bytes) {
        next_name[name_bytes] = second->name[name_bytes];
        if (next_name[name_bytes++] == 0) { break; }
    }
    if (name_bytes == 0 || next_name[name_bytes - 1] != 0) { return 194; }
    long current = call(8, root, 0, 1, 0);
    if (current < prefix_cookie || call(8, alias, 0, 1, 0) != current) { return 195; }
    long cookie = (long)first->next;
    if (cookie < 0 || call(8, root, cookie, 0, 0) != cookie
        || call(217, alias, (word)buffer, sizeof(buffer), 0) <= 0) { return 195; }
    first = (void *)buffer;
    if (first->bytes < 19 + name_bytes) { return 195; }
    for (word i = 0; i < name_bytes; ++i) { if (first->name[i] != next_name[i]) { return 195; } }
    if (call(8, root, 0, 0, 0) != 0
        || call(217, alias, (word)buffer, sizeof(buffer), 0) <= 0) { return 196; }
    current = call(8, root, 0, 1, 0);
    if (current != prefix_cookie || call(217, root, 1, sizeof(buffer), 0) != -14
        || call(8, alias, 0, 1, 0) != current) { return 196; }
    if (call(3, alias, 0, 0, 0) != 0 || call(3, root, 0, 0, 0) != 0) { return 196; }
    return 0;
}

static int unix_stream_append_cases(void) {
    int sockets[2];
    unsigned char output[1024];
    if (call(53, 1, 1 | 0x800, 0, (word)sockets) != 0) { return 197; }
    if (call(1, sockets[0], (word)"prefix", 6, 0) != 6
        || call(0, sockets[1], (word)output, 2, 0) != 2
        || output[0] != 'p' || output[1] != 'r') { return 197; }
    struct { word base; word len; } iov[2] = {{(word)"a", 1}, {1, 1}};
    if (call(20, sockets[0], (word)iov, 2, 0) != -14) { return 198; }
    iov[1].base = (word)"b";
    if (call(20, sockets[0], (word)iov, 2, 0) != 2
        || call(0, sockets[1], (word)output, 6, 0) != 6) { return 198; }
    for (word i = 0; i < 6; ++i) { if (output[i] != (unsigned char)"efixab"[i]) { return 198; } }
    for (word i = 0; i < 128; ++i) {
        unsigned char record[8];
        for (word j = 0; j < 8; ++j) { record[j] = (unsigned char)i; }
        if (call(1, sockets[0], (word)record, sizeof(record), 0) != sizeof(record)) { return 199; }
    }
    if (call(0, sockets[1], (word)output, sizeof(output), 0) != sizeof(output)) { return 199; }
    for (word i = 0; i < sizeof(output); ++i) { if (output[i] != (unsigned char)(i / 8)) { return 199; } }
    if (call(3, sockets[0], 0, 0, 0) != 0 || call(3, sockets[1], 0, 0, 0) != 0) { return 199; }
    return 0;
}

static int rooted_open_cases(void) {
    static const char dir[] = "/tmp/nanami-rooted-open";
    static const char file[] = "/tmp/nanami-rooted-open/file";
    static const char link[] = "/tmp/nanami-rooted-open/link";
    static const char root_link[] = "/tmp/nanami-rooted-open/root-link";
    static const char dev_link[] = "/tmp/nanami-rooted-open/dev-link";
    static const char data[] = "rooted-open";
    unsigned char output[32];
    long descriptors[64];
    call(87, (word)link, 0, 0, 0);
    call(87, (word)root_link, 0, 0, 0);
    call(87, (word)dev_link, 0, 0, 0);
    call(87, (word)file, 0, 0, 0);
    long result = call(83, (word)dir, 0700, 0, 0);
    if (result != 0 && result != -17) { return 170; }
    long fd = call(2, (word)file, 01102, 0600, 0);
    if (fd < 0 || call(1, fd, (word)data, sizeof(data), 0) != sizeof(data)
        || call(3, fd, 0, 0, 0) != 0) { return 171; }
    fd = call(2, (word)file, 02000000, 0, 0);
    if (fd < 0 || call(72, fd, 1, 0, 0) != 1
        || call(0, fd, (word)output, sizeof(output), 0) != sizeof(data)) { return 172; }
    for (word i = 0; i < sizeof(data); ++i) { if (output[i] != (unsigned char)data[i]) { return 173; } }
    call(3, fd, 0, 0, 0);
    long directory = call(2, (word)dir, 0200000, 0, 0);
    if (directory < 0) { return 174; }
    fd = call(257, directory, (word)"file", 0, 0);
    if (fd < 0 || call(0, fd, (word)output, sizeof(output), 0) != sizeof(data)) { return 175; }
    call(3, fd, 0, 0, 0);
    if (call(257, 999, (word)"file", 0, 0) != -9) { return 176; }
    fd = call(257, 999, (word)file, 0400000, 0);
    if (fd < 0) { return 177; }
    call(3, fd, 0, 0, 0);
    if (call(88, (word)"file", (word)link, 0, 0) != 0
        || call(2, (word)link, 0400000, 0, 0) != -40) { return 178; }
    fd = call(2, (word)link, 0, 0, 0);
    if (fd < 0 || call(0, fd, (word)output, sizeof(output), 0) != sizeof(data)) { return 179; }
    call(3, fd, 0, 0, 0);
    if (call(87, (word)link, 0, 0, 0) != 0
        || call(88, (word)"absent", (word)link, 0, 0) != 0
        || call(2, (word)link, 0, 0, 0) != -2) { return 180; }
    if (call(88, (word)"/", (word)root_link, 0, 0) != 0) { return 181; }
    fd = call(2, (word)root_link, 0200000, 0, 0);
    if (fd < 0) { return 182; }
    call(3, fd, 0, 0, 0);
    fd = call(2, (word)file, 0200000, 0, 0);
    if (fd >= 0) { return 183; }
    if (call(88, (word)"/dev", (word)dev_link, 0, 0) != 0) { return 189; }
    fd = call(2, (word)"/dev/null", 0, 0, 0);
    if (fd < 0 || call(0, fd, (word)output, 1, 0) != 0) { return 184; }
    call(3, fd, 0, 0, 0);
    if (call(2, 1, 0, 0, 0) != -14) { return 185; }
    char cwd[256];
    if (call(80, (word)root_link, 0, 0, 0) != 0
        || call(80, (word)dir, 0, 0, 0) != 0
        || call(79, (word)cwd, sizeof(cwd), 0, 0) != sizeof(dir)) { return 190; }
    for (word i = 0; i < sizeof(dir); ++i) { if (cwd[i] != dir[i]) { return 191; } }
    if (call(80, (word)file, 0, 0, 0) != -20
        || call(80, (word)link, 0, 0, 0) != -2
        || call(280, directory, (word)"file", 0, 0) != 0
        || call(280, directory, (word)"absent", 0, 0) != -2
        || call(280, directory, (word)"file", 0, 0x8000) != -22
        || call(80, (word)"/", 0, 0, 0) != 0) { return 192; }
    long spare = call(2, (word)"/dev", 0200000, 0, 0);
    if (spare < 0) { return 188; }
    for (word round = 0; round < 3; ++round) {
        word count = 0;
        while (count < 64) {
            fd = call(2, (word)file, 0, 0, 0);
            if (fd < 0) { break; }
            descriptors[count++] = fd;
        }
        if (count == 0) { return 186; }
        /* Resolve the disk symlink before entering the personality-owned /dev.
           Release a pure-virtual spare so a Linux slot is available even if
           its own limit was reached. /dev needs no backing POSIX descriptor. */
        if (call(3, spare, 0, 0, 0) != 0) { return 188; }
        long virtual_directory = call(2, (word)dev_link, 0200000, 0, 0);
        if (virtual_directory < 0) { return 188; }
        spare = virtual_directory;
        for (word i = 0; i < count; ++i) {
            if (call(3, descriptors[i], 0, 0, 0) != 0) { return 187; }
        }
    }
    call(3, spare, 0, 0, 0);
    call(3, directory, 0, 0, 0);
    call(87, (word)root_link, 0, 0, 0);
    call(87, (word)dev_link, 0, 0, 0);
    call(87, (word)link, 0, 0, 0);
    call(87, (word)file, 0, 0, 0);
    call(84, (word)dir, 0, 0, 0);
    static const char pass[] = "ROOTED-OPEN-PASS\n";
    call(1, 1, (word)pass, sizeof(pass) - 1, 0);
    return 0;
}

static int readlink_kind_cases(void) {
    static const char file[] = "/tmp/nanami-readlink-kind-file";
    static const char link[] = "/tmp/nanami-readlink-kind-link";
    static const char sock[] = "/tmp/nanami-readlink-kind.sock";
    static const char target[] = "nanami-readlink-kind-file";
    static const char absent[] = "/tmp/nanami-readlink-absent";
    unsigned char output[64];
    for (word i = 0; i < sizeof(output); ++i) { output[i] = 0xa5; }
    call(87, (word)file, 0, 0, 0);
    call(87, (word)link, 0, 0, 0);
    call(87, (word)sock, 0, 0, 0);
    call(87, (word)absent, 0, 0, 0);
    long fd = call(2, (word)file, 0102, 0600, 0);
    if (fd < 0 || call(3, fd, 0, 0, 0) != 0) { return 150; }
    if (call(89, (word)file, (word)output, 64, 0) != -22
        || call(89, (word)file, 0, 1, 0) != -14
        || call(89, (word)file, 1, 1, 0) != -22
        || call(89, (word)file, 0, 0, 0) != -22
        || call(89, (word)"/tmp", (word)output, 64, 0) != -22
        || call(89, (word)absent, (word)output, 64, 0) != -2) { return 151; }
    for (word i = 0; i < sizeof(output); ++i) { if (output[i] != 0xa5) { return 152; } }
    if (call(88, (word)target, (word)link, 0, 0) != 0) { return 153; }
    if (call(89, (word)link, (word)output, 3, 0) != 3
        || output[0] != 'n' || output[1] != 'a' || output[2] != 'n' || output[3] != 0xa5) { return 154; }
    if (call(89, (word)link, 0, 1, 0) != -14 || call(89, (word)link, (word)output, 0, 0) != -22) { return 155; }
    long dir = call(2, (word)"/tmp", 0200000, 0, 0);
    if (dir < 0 || call(267, dir, (word)"nanami-readlink-kind-file", (word)output, 64) != -22
        || call(267, dir, (word)"nanami-readlink-kind-link", (word)output, 3) != 3
        || call(267, 999, (word)"nanami-readlink-kind-link", (word)output, 3) != -9
        || call(267, 999, (word)link, (word)output, 3) != 3) { return 156; }
    call(3, dir, 0, 0, 0);
    readlink_state[0] = 0x5a;
    if (call(10, (word)readlink_state, 4096, 1, 0) != 0
        || call(89, (word)file, (word)readlink_state, 1, 0) != -22
        || call(89, (word)link, (word)readlink_state, 1, 0) != -14
        || readlink_state[0] != 0x5a
        || call(10, (word)readlink_state, 4096, 3, 0) != 0) { return 157; }
    if (call(87, (word)file, 0, 0, 0) != 0 || call(88, (word)"missing-target", (word)file, 0, 0) != 0
        || call(89, (word)file, (word)output, 64, 0) != 14 || output[0] != 'm') { return 158; }
    if (call(87, (word)file, 0, 0, 0) != 0) { return 159; }
    fd = call(2, (word)file, 0102, 0600, 0);
    if (fd < 0 || call(3, fd, 0, 0, 0) != 0 || call(89, (word)file, (word)output, 64, 0) != -22) { return 160; }
    long length = call(89, (word)"/proc/self", (word)output, 64, 0);
    if (length <= 0 || length > 20 || output[0] < '0' || output[0] > '9') { return 161; }
    length = call(89, (word)"/proc/self/exe", (word)output, 64, 0);
    if (length <= 0 || output[0] != '/') { return 162; }
    unsigned char address[110] = {1, 0};
    for (word i = 0; i < sizeof(sock); ++i) { address[i + 2] = sock[i]; }
    fd = call(41, 1, 1, 0, 0);
    if (fd < 0 || call(49, fd, (word)address, sizeof(sock) + 2, 0) != 0
        || call(89, (word)sock, (word)output, 64, 0) != -22
        || call(89, (word)sock, 0, 1, 0) != -14) { return 163; }
    call(3, fd, 0, 0, 0);
    call(87, (word)sock, 0, 0, 0);
    call(87, (word)link, 0, 0, 0);
    call(87, (word)file, 0, 0, 0);
    static const char pass[] = "READLINK-KIND-PASS\n";
    call(1, 1, (word)pass, sizeof(pass) - 1, 0);
    return 0;
}

static void recv_field_store(volatile unsigned char *p, word value, word bytes) {
    for (word i = 0; i < bytes; ++i) {
        p[i] = (unsigned char)(value >> (i * 8));
    }
}

static word recv_field_load(const volatile unsigned char *p, word bytes) {
    word value = 0;
    for (word i = 0; i < bytes; ++i) {
        value |= (word)p[i] << (i * 8);
    }
    return value;
}

static int recv_queue_is_intact(int socket, volatile unsigned char *buffer) {
    if (linux_call(45, socket, (word)buffer, 9, 2, 0, 0) != 9) { return 0; }
    for (word i = 0; i < 9; ++i) {
        if (buffer[i] != (unsigned char)"abcdefghi"[i]) { return 0; }
    }
    return 1;
}

/* A readable but non-writable second header page permits msghdr copyin while
 * failing each whole output field, including fields straddling the boundary.
 * Byte-wise header access also tests unaligned msghdr without C alignment UB. */
static int unix_coalesced_copyout_faults(void) {
    static const word boundaries[] = {8, 10, 40, 43, 44, 47, 48, 51};
    for (word peek = 0; peek <= 2; peek += 2) {
        for (word test = 0; test < sizeof(boundaries) / sizeof(boundaries[0]); ++test) {
            long mapped = linux_call(9, 0, 3 * 4096, 3, 0x22, -1UL, 0);
            int sockets[2];
            if (mapped < 0 || linux_call(53, 1, 1 | 0x800, 0, (word)sockets, 0, 0) != 0) {
                return 130;
            }
            volatile unsigned char *pages = (void *)mapped;
            volatile unsigned char *header = pages + 4096 - boundaries[test];
            volatile unsigned char *payload = pages + 2 * 4096;
            struct { word base; word len; } iov = {(word)payload, 7};
            for (word i = 0; i < 56; ++i) { header[i] = 0xa5; }
            for (word i = 0; i < 9; ++i) { payload[i] = 0; }
            payload[9] = 0x7e;
            recv_field_store(header, 0, 8);
            recv_field_store(header + 8, 99, 4);
            recv_field_store(header + 16, (word)&iov, 8);
            recv_field_store(header + 24, 1, 8);
            recv_field_store(header + 32, 0, 8);
            recv_field_store(header + 40, 24, 8);
            recv_field_store(header + 48, 123, 4);
            if (call(1, sockets[0], (word)"abc", 3, 0) != 3
                || call(1, sockets[0], (word)"def", 3, 0) != 3
                || call(1, sockets[0], (word)"ghi", 3, 0) != 3
                || call(10, mapped + 4096, 4096, 1, 0) != 0) { return 131; }
            if (call(47, sockets[1], (word)header, peek, 0) != -14) { return 132; }
            for (word i = 0; i < 7; ++i) {
                if (payload[i] != (unsigned char)"abcdefg"[i]) { return 133; }
            }
            if (payload[7] != 0 || payload[8] != 0 || payload[9] != 0x7e
                || recv_field_load(header + 8, 4) != (boundaries[test] <= 10 ? 99UL : 0UL)
                || recv_field_load(header + 40, 8) != (boundaries[test] <= 47 ? 24UL : 0UL)
                || recv_field_load(header + 48, 4) != 123) { return 134; }
            for (word i = 12; i < 16; ++i) {
                if (header[i] != 0xa5 || header[i + 40] != 0xa5) { return 135; }
            }
            if (!recv_queue_is_intact(sockets[1], payload + 16)
                || call(10, mapped + 4096, 4096, 3, 0) != 0) { return 136; }
            if (call(47, sockets[1], (word)header, 2, 0) != 7
                || !recv_queue_is_intact(sockets[1], payload + 16)
                || recv_field_load(header + 8, 4) != 0
                || recv_field_load(header + 40, 8) != 0
                || recv_field_load(header + 48, 4) != 0) { return 137; }
            if (call(47, sockets[1], (word)header, 0, 0) != 7
                || call(0, sockets[1], (word)(payload + 7), 2, 0) != 2
                || payload[7] != 'h' || payload[8] != 'i' || payload[9] != 0x7e
                || call(0, sockets[1], (word)payload, 1, 0) != -11) { return 138; }
            if (call(3, sockets[0], 0, 0, 0) != 0 || call(3, sockets[1], 0, 0, 0) != 0
                || call(11, mapped, 3 * 4096, 0, 0) != 0) { return 139; }
        }
    }
    static const char pass[] = "UNIX-RECVMSG-FAULT-PASS\n";
    call(1, 1, (word)pass, sizeof(pass) - 1, 0);
    return 0;
}

static int read_named_byte(const char *path, unsigned char expected) {
    unsigned char byte = 0;
    long fd = call(2, (word)path, 0, 0, 0);
    if (fd < 0) { return 0; }
    long count = call(0, fd, (word)&byte, 1, 0);
    long closed = call(3, fd, 0, 0, 0);
    return count == 1 && closed == 0 && byte == expected;
}

/* Exercise syscall copyin through real guest page tables, including a working
 * set larger than the bounded read window. Both cached and checked-copy builds
 * must observe the current backing and permissions, never a byte snapshot. */
static int syscall_read_backing(void) {
    static const char names[2][24] = {"/tmp/read-window-a", "/tmp/read-window-b"};
    for (word i = 0; i < 2; ++i) {
        unsigned char value = (unsigned char)('a' + i);
        long fd = call(2, (word)names[i], 0102, 0600, 0);
        if (fd < 0 || call(1, fd, (word)&value, 1, 0) != 1
            || call(3, fd, 0, 0, 0) != 0) { return 60; }
    }
    word bytes = 80 * 4096;
    long mapped = linux_call(9, 0, bytes, 3, 0x22, -1UL, 0);
    if (mapped < 0) { return 61; }
    char *paths = (void *)mapped;
    for (word page = 0; page < bytes; page += 4096) {
        for (word i = 0; i < sizeof(names[0]); ++i) { paths[page + i] = names[0][i]; }
        if (!read_named_byte(paths + page, 'a')) { return 62; }
    }
    for (word i = 0; i < sizeof(names[1]); ++i) { paths[i] = names[1][i]; }
    if (!read_named_byte(paths, 'b')) { return 63; }
    if (call(10, mapped, 4096, 0, 0) != 0
        || call(2, mapped, 0, 0, 0) != -14
        || call(10, mapped, 4096, 1, 0) != 0
        || !read_named_byte(paths, 'b')) { return 64; }
    long child = call(57, 0, 0, 0, 0);
    if (child < 0) { return 65; }
    if (child == 0) {
        if (call(10, mapped, 4096, 3, 0) != 0) { return 66; }
        for (word i = 0; i < sizeof(names[0]); ++i) { paths[i] = names[0][i]; }
        if (!read_named_byte(paths, 'a')) { return 67; }
        call(60, 0, 0, 0, 0);
        return 68;
    }
    int status = -1;
    if (call(61, child, (word)&status, 0, 0) != child || status != 0
        || !read_named_byte(paths, 'b')) { return 69; }
    if (call(11, mapped, bytes, 0, 0) != 0) { return 70; }
    long replacement = linux_call(9, mapped, 4096, 3, 0x32, -1UL, 0);
    if (replacement != mapped) { return 71; }
    paths = (void *)replacement;
    for (word i = 0; i < sizeof(names[0]); ++i) { paths[i] = names[0][i]; }
    if (!read_named_byte(paths, 'a') || call(11, replacement, 4096, 0, 0) != 0) { return 72; }
    for (word i = 0; i < 2; ++i) {
        if (call(87, (word)names[i], 0, 0, 0) != 0) { return 73; }
    }
    return 0;
}

static int private_mapping_and_fork(void) {
    static unsigned char data[5003];
    for (word i = 0; i < sizeof(data); ++i) { data[i] = (unsigned char)(i % 239); }
    const char *name = "/tmp/private-file-test";
    long fd = call(2, (word)name, 0102, 0600, 0);
    if (fd < 0 || call(1, fd, (word)data, sizeof(data), 0) != sizeof(data)) { return 40; }
    long first = linux_call(9, 0, 8192, 3, 2, fd, 0);
    long second = linux_call(9, 0, 8192, 1, 2, fd, 0);
    if (first < 0 || second < 0) { return 41; }
    volatile unsigned char *a = (void *)first;
    volatile unsigned char *b = (void *)second;
    for (word i = 0; i < 8192; ++i) {
        unsigned char expected = 0;
        if (i < sizeof(data)) { expected = data[i]; }
        if (a[i] != expected || b[i] != expected) { return 42; }
    }
    /* Force the original snapshot out of the bounded cache while this mapping
       remains live. Reusing its native VA must not refill the guest's pages. */
    for (word item = 0; item < 52; ++item) {
        char path[] = "/tmp/snapshot-00";
        path[14] = (char)('0' + item / 10);
        path[15] = (char)('0' + item % 10);
        static unsigned char page[4096];
        for (word i = 0; i < sizeof(page); ++i) { page[i] = (unsigned char)(item + 1); }
        long other = call(2, (word)path, 0102, 0600, 0);
        if (other < 0 || call(1, other, (word)page, sizeof(page), 0) != sizeof(page)) { return 74; }
        long transient = linux_call(9, 0, sizeof(page), 1, 2, other, 0);
        if (transient < 0 || *(volatile unsigned char *)transient != item + 1
            || call(11, transient, sizeof(page), 0, 0) != 0
            || call(3, other, 0, 0, 0) != 0 || call(87, (word)path, 0, 0, 0) != 0) { return 75; }
        if (b[1] != data[1] || b[5002] != data[5002] || b[8191] != 0) { return 76; }
    }
    a[0] = 0x51;
    protected_state[0] = 0x37;
    if (call(10, (word)protected_state, 4096, 1, 0) != 0
        || call(10, first, 8192, 0, 0) != 0) { return 43; }
    /* The running image is removed only from this QEMU snapshot. Fork must
       clone the current VM rather than reopen / reload the executable. */
    if (call(87, (word)"/bin/nanami-file-mapping", 0, 0, 0) != 0) { return 44; }
    long child = call(57, 0, 0, 0, 0);
    if (child < 0) { return 45; }
    if (child == 0) {
        if (call(10, first, 8192, 3, 0) != 0 || a[0] != 0x51
            || b[1] != data[1] || a[5003] != 0 || a[8191] != 0
            || protected_state[0] != 0x37) { return 46; }
        a[0] = 0x52;
        if (call(10, second, 8192, 3, 0) != 0 || b[1] != data[1]) { return 77; }
        b[1] = 0xee;
        call(60, 0, 0, 0, 0);
        return 57;
    }
    int status = -1;
    if (call(61, child, (word)&status, 0, 0) != child || status != 0) { return 47; }
    if (call(10, first, 8192, 3, 0) != 0 || a[0] != 0x51
        || protected_state[0] != 0x37 || b[1] != data[1]) { return 48; }
    /* Replace old dirty backing and check the partial EOF page again. */
    if (linux_call(9, first, 8192, 1, 2 | 0x10, fd, 0) != first
        || a[0] != data[0] || a[5002] != data[5002] || a[8191] != 0) { return 49; }
    if (call(11, first, 8192, 0, 0) != 0 || call(11, second, 8192, 0, 0) != 0) { return 50; }
    call(3, fd, 0, 0, 0);
    call(87, (word)name, 0, 0, 0);
    return 0;
}
static int directory_with_unrelated_appends(void) {
    const char *directory = "/tmp/directory-progress";
    const char *log = "/tmp/directory-progress-log";
    char path[] = "/tmp/directory-progress/f00";
    if (call(83, (word)directory, 0700, 0, 0) != 0) { return 215; }
    for (word i = 0; i < 64; ++i) {
        path[sizeof(path) - 3] = '0' + i / 10;
        path[sizeof(path) - 2] = '0' + i % 10;
        long fd = call(2, (word)path, 01102, 0600, 0);
        if (fd < 0 || call(3, fd, 0, 0, 0) != 0) { return 216; }
    }
    long fd = call(2, (word)directory, 0200000, 0, 0);
    long writer = call(2, (word)log, 01102, 0600, 0);
    unsigned char value = 0x31;
    if (fd < 0 || writer < 0 || call(1, writer, (word)&value, 1, 0) != 1) { return 217; }
    unsigned char seen[64] = {0};
    unsigned char buffer[128];
    word count = 0;
    for (;;) {
        long bytes = call(217, fd, (word)buffer, sizeof(buffer), 0);
        if (bytes < 0) { return 218; }
        if (bytes == 0) { break; }
        word byte_count = (word)bytes;
        word offset = 0;
        while (offset < byte_count) {
            struct directory_record *entry = (void *)(buffer + offset);
            if (entry->bytes < 24 || offset + entry->bytes > byte_count) { return 218; }
            if (entry->name[0] != '.') {
                if (entry->name[0] != 'f' || entry->name[1] < '0' || entry->name[1] > '6'
                    || entry->name[2] < '0' || entry->name[2] > '9' || entry->name[3] != 0) { return 219; }
                word index = (entry->name[1] - '0') * 10 + entry->name[2] - '0';
                if (index >= 64 || seen[index] != 0) { return 219; }
                seen[index] = 1;
                count += 1;
            }
            offset += entry->bytes;
        }
        if (call(1, writer, (word)&value, 1, 0) != 1) { return 220; }
    }
    if (count != 64 || call(74, writer, 0, 0, 0) != 0
        || call(3, writer, 0, 0, 0) != 0 || call(3, fd, 0, 0, 0) != 0) { return 221; }
    for (word i = 0; i < 64; ++i) {
        path[sizeof(path) - 3] = '0' + i / 10;
        path[sizeof(path) - 2] = '0' + i % 10;
        if (call(87, (word)path, 0, 0, 0) != 0) { return 222; }
    }
    if (call(84, (word)directory, 0, 0, 0) != 0 || call(87, (word)log, 0, 0, 0) != 0) { return 223; }
    return 0;
}

static int append_existing_blocks(void) {
    static unsigned char seed[13 * 1024 + 960];
    static unsigned char output[sizeof(seed) + 128 * 8];
    unsigned char tail[8];
    for (word i = 0; i < sizeof(seed); ++i) { seed[i] = (unsigned char)(i % 191 + 1); }
    const char *name = "/tmp/append-existing-blocks";
    long fd = call(2, (word)name, 01102, 0600, 0);
    if (fd < 0 || call(1, fd, (word)seed, sizeof(seed), 0) != sizeof(seed)) { return 209; }
    /* Begin inside an existing single-indirect data block, cross its end,
     * then continue inside the newly allocated block. Each call still commits
     * its payload and inode size; only allocation-changing calls recount. */
    for (word iteration = 0; iteration < 128; ++iteration) {
        for (word i = 0; i < sizeof(tail); ++i) { tail[i] = (unsigned char)(iteration + i); }
        if (call(1, fd, (word)tail, sizeof(tail), 0) != sizeof(tail)) { return 210; }
    }
    if (call(74, fd, 0, 0, 0) != 0 || call(3, fd, 0, 0, 0) != 0) { return 211; }
    fd = call(2, (word)name, 0, 0, 0);
    if (fd < 0 || call(8, fd, 0, 2, 0) != sizeof(output)
        || call(17, fd, (word)output, sizeof(output), 0) != sizeof(output)) { return 212; }
    for (word i = 0; i < sizeof(output); ++i) {
        unsigned char expected;
        if (i < sizeof(seed)) { expected = seed[i]; }
        else {
            word offset = i - sizeof(seed);
            expected = (unsigned char)(offset / 8 + offset % 8);
        }
        if (output[i] != expected) { return 213; }
    }
    if (call(3, fd, 0, 0, 0) != 0 || call(87, (word)name, 0, 0, 0) != 0) { return 214; }
    return 0;
}

static int snapshot_write_prepare_and_fixed_reload(void) {
    static unsigned char data[5003];
    const char *name = "/tmp/writable-fixed-snapshot";
    for (word i = 0; i < sizeof(data); ++i) { data[i] = (unsigned char)(i % 193 + 1); }
    long fd = call(2, (word)name, 01102, 0600, 0);
    if (fd < 0 || call(1, fd, (word)data, sizeof(data), 0) != sizeof(data)) { return 239; }
    long peer = linux_call(9, 0, 8192, 1, 2, fd, 0);
    long address = linux_call(9, 0, 12288, 1, 2, fd, 0);
    if (peer < 0 || address < 0) { return 240; }
    volatile unsigned char *p = (void *)address;
    volatile unsigned char *q = (void *)peer;
    if (call(10, address + 4096, 4096, 3, 0) != 0) { return 241; }
    for (word i = 0; i < 12288; ++i) {
        if (p[i] != (i < sizeof(data) ? data[i] : 0)) { return 242; }
    }
    p[4096] = 0x96;
    p[8191] = 0x7e;
    unsigned char disk = 0;
    if (q[4096] != data[4096] || q[8191] != 0
        || call(17, fd, (word)&disk, 1, 4096) != 1 || disk != data[4096]) { return 243; }
    /* A readonly downgrade must not resurrect an immutable origin for bytes
     * modified after mprotect. A later fixed read reloads. */
    if (call(10, address + 4096, 4096, 1, 0) != 0
        || linux_call(9, address + 4096, 4096, 1, 18, fd, 4096) != address + 4096
        || p[4096] != data[4096] || p[8191] != 0) { return 244; }
    if (call(11, address, 12288, 0, 0) != 0 || call(11, peer, 8192, 0, 0) != 0
        || call(3, fd, 0, 0, 0) != 0 || call(87, (word)name, 0, 0, 0) != 0) { return 245; }
    static const char pass[] = "TINYCORE-SNAPSHOT-WRITE-PREPARE-PASS\n";
    call(1, 1, (word)pass, sizeof(pass) - 1, 0);
    return 0;
}

static int readonly_snapshot_tail_and_retirement(void) {
    static unsigned char data[5003];
    static unsigned char churn[4096];
    for (word i = 0; i < sizeof(data); ++i) { data[i] = (unsigned char)(i % 193 + 1); }
    const char *name = "/tmp/snapshot-tail-file";
    const char *other = "/tmp/snapshot-churn-file";
    long fd = call(2, (word)name, 01102, 0600, 0);
    if (fd < 0 || call(1, fd, (word)data, sizeof(data), 0) != sizeof(data)) { return 200; }
    long address = linux_call(9, 0, 12288, 1, 2, fd, 0);
    if (address < 0) { return 201; }
    volatile unsigned char *p = (void *)address;
    long scratch = call(2, (word)other, 01102, 0600, 0);
    if (scratch < 0) { return 202; }
    /* Same-inode generation churn verifies freshness. Replacement may retain
     * unrelated snapshots, so distinct live inodes below force retirement. */
    for (word version = 0; version < 50; ++version) {
        for (word i = 0; i < sizeof(churn); ++i) { churn[i] = (unsigned char)(version + 3); }
        if (call(18, scratch, (word)churn, sizeof(churn), 0) != sizeof(churn)) { return 202; }
        long temporary = linux_call(9, 0, 4096, 1, 2, scratch, 0);
        if (temporary < 0 || *(volatile unsigned char *)temporary != churn[0]
            || call(11, temporary, 4096, 0, 0) != 0) { return 203; }
    }
    char distinct[] = "/tmp/snapshot-distinct-00";
    for (word file = 0; file < 50; ++file) {
        distinct[sizeof(distinct) - 3] = '0' + file / 10;
        distinct[sizeof(distinct) - 2] = '0' + file % 10;
        long item = call(2, (word)distinct, 01102, 0600, 0);
        for (word i = 0; i < sizeof(churn); ++i) { churn[i] = (unsigned char)(0x60 + file); }
        if (item < 0 || call(1, item, (word)churn, sizeof(churn), 0) != sizeof(churn)) { return 224; }
        long temporary = linux_call(9, 0, 4096, 1, 2, item, 0);
        if (temporary < 0 || *(volatile unsigned char *)temporary != churn[0]
            || *(volatile unsigned char *)(temporary + 4095) != churn[4095]
            || call(11, temporary, 4096, 0, 0) != 0 || call(3, item, 0, 0, 0) != 0) { return 225; }
        /* Keep directory links live so ext2 cannot reuse one inode number. */
    }
    for (word i = 0; i < 12288; ++i) {
        unsigned char expected = 0;
        if (i < sizeof(data)) { expected = data[i]; }
        if (p[i] != expected) { return 204; }
    }
    long child = call(57, 0, 0, 0, 0);
    if (child < 0) { return 205; }
    if (child == 0) {
        int result = 0;
        if (call(10, address, 12288, 3, 0) != 0) { result = 206; }
        if (result == 0) {
            p[0] = 0x81;
            p[8192] = 0x82;
            if (p[0] != 0x81 || p[8192] != 0x82) { result = 206; }
        }
        call(60, (word)result, 0, 0, 0);
        for (;;) {}
    }
    int status = -1;
    if (call(61, child, (word)&status, 0, 0) != child || status != 0
        || p[0] != data[0] || p[8192] != 0
        || call(10, address, 12288, 3, 0) != 0) { return 207; }
    p[4096] = 0x83;
    p[12287] = 0x84;
    unsigned char disk = 0;
    if (call(17, fd, (word)&disk, 1, 4096) != 1 || disk != data[4096]
        || call(17, fd, (word)&disk, 1, 8192) != 0
        || call(11, address, 12288, 0, 0) != 0) { return 208; }
    call(3, scratch, 0, 0, 0);
    call(3, fd, 0, 0, 0);
    call(87, (word)other, 0, 0, 0);
    call(87, (word)name, 0, 0, 0);
    for (word file = 0; file < 50; ++file) {
        distinct[sizeof(distinct) - 3] = '0' + file / 10;
        distinct[sizeof(distinct) - 2] = '0' + file % 10;
        if (call(87, (word)distinct, 0, 0, 0) != 0) { return 226; }
    }
    return 0;
}

static int uncached_private_mapping(void) {
    static unsigned char block[65536];
    for (word i = 0; i < sizeof(block); ++i) { block[i] = 0x4d; }
    const char *name = "/tmp/large-private-file-test";
    long fd = call(2, (word)name, 0102, 0600, 0);
    if (fd < 0) { return 51; }
    for (word i = 0; i < 32; ++i) {
        if (call(1, fd, (word)block, sizeof(block), 0) != sizeof(block)) { return 52; }
    }
    if (call(1, fd, (word)block, 123, 0) != 123) { return 53; }
    /* Overwrite existing blocks across direct/single/double pointer-table
     * boundaries, including partial edge blocks. The full mapping below checks
     * payload and untouched neighbors after range lookahead uses scratch. */
    const word overwrite_offsets[] = {11 * 1024, 267 * 1024, 523 * 1024 + 7};
    for (word i = 0; i < sizeof(block); ++i) { block[i] = 0x6e; }
    for (word i = 0; i < 3; ++i) {
        if (call(18, fd, (word)block, sizeof(block), overwrite_offsets[i]) != sizeof(block)) { return 57; }
    }
    word bytes = 2 * 1024 * 1024;
    long address = linux_call(9, 0, bytes + 4096, 1, 2, fd, 0);
    if (address < 0) { return 54; }
    volatile unsigned char *a = (void *)address;
    for (word i = 0; i < bytes + 4096; ++i) {
        unsigned char expected = 0;
        if (i < bytes + 123) { expected = 0x4d; }
        for (word range = 0; range < 3; ++range) {
            if (i >= overwrite_offsets[range] && i < overwrite_offsets[range] + sizeof(block)) {
                expected = 0x6e;
            }
        }
        if (a[i] != expected) { return 55; }
    }
    if (call(11, address, bytes + 4096, 0, 0) != 0) { return 56; }
    address = linux_call(9, 0, bytes + 4096, 3, 2, fd, 0);
    if (address < 0) { return 58; }
    a = (void *)address;
    if (a[0] != 0x4d || a[bytes + 122] != 0x4d || a[bytes + 123] != 0) { return 58; }
    a[0] = 0x71;
    a[bytes + 122] = 0x72;
    unsigned char disk_byte = 0;
    if (call(17, fd, (word)&disk_byte, 1, 0) != 1 || disk_byte != 0x4d
        || call(17, fd, (word)&disk_byte, 1, bytes + 122) != 1 || disk_byte != 0x4d
        || call(11, address, bytes + 4096, 0, 0) != 0) { return 59; }
    call(3, fd, 0, 0, 0);
    call(87, (word)name, 0, 0, 0);
    static const char pass[] = "BLOCK-MAP-RUN-PASS\n";
    call(1, 1, (word)pass, sizeof(pass) - 1, 0);
    return 0;
}
int test_main(void) {
    int stack_result = exec_demand_stack_cases();
    if (stack_result != 0) { return stack_result; }
    stack_result = brk_collision_cases();
    if (stack_result != 0) { return stack_result; }
    stack_result = demand_stack_cases();
    if (stack_result != 0) { return stack_result; }
    stack_result = shared_demand_stack_cases();
    if (stack_result != 0) { return stack_result; }
    static char data[8192];
    for (word i = 0; i < sizeof(data); ++i) { data[i] = (char)(i % 121); }
    const char *name = "/tmp/shared-file-test";
    long fd = call(2, (word)name, 0102, 0600, 0);
    if (fd < 0 || call(1, fd, (word)data, sizeof(data), 0) != sizeof(data)) { return 10; }
    long ro = call(2, (word)name, 0, 0, 0);
    long first = linux_call(9, 0, 5000, 1, 1, ro, 0);
    long other = linux_call(9, 0, 4096, 1, 1, fd, 4096);
    if (first < 0 || other < 0) { return 11; }
    volatile unsigned char *a = (void *)first;
    volatile unsigned char *b = (void *)other;
    if (a[4097] != (unsigned char)data[4097] || b[1] != a[4097]) { return 12; }
    if (call(3, ro, 0, 0, 0) != 0) { return 13; }
    if (call(10, first, 4096, 3, 0) != -13) { return 14; }
    if (linux_call(9, 0, 4096, 3, 1, fd, 0) != -95) { return 15; }
    char value = 0x7b;
    if (call(18, fd, (word)&value, 1, 4097) != 1 || a[4097] != 0x7b || b[1] != 0x7b) { return 16; }
    if (call(77, fd, 0, 0, 0) != -16) { return 17; }
    long child = call(57, 0, 0, 0, 0);
    if (child < 0) { return 18; }
    if (child == 0) {
        if (a[4097] != 0x7b || b[1] != 0x7b) { return 19; }
        value = 0x7c;
        if (call(18, fd, (word)&value, 1, 4097) != 1) { return 20; }
        return 0;
    }
    int status = -1;
    if (call(61, child, (word)&status, 0, 0) != child || status != 0 || a[4097] != 0x7c || b[1] != 0x7c) { return 21; }
    if (call(26, first, 5000, 4, 0) != 0) { return 30; }
    if (call(87, (word)name, 0, 0, 0) != 0 || call(3, fd, 0, 0, 0) != 0) { return 22; }
    if (call(11, first, 4096, 0, 0) != 0 || a[4097] != 0x7c) { return 23; }
    if (call(11, first + 4096, 4096, 0, 0) != 0 || b[1] != 0x7c) { return 24; }
    if (call(11, other, 4096, 0, 0) != 0) { return 25; }
    /* After the last unmap, the hidden FD and VFS page leases must be gone. */
    fd = call(2, (word)name, 0102, 0600, 0);
    if (fd < 0 || call(77, fd, 5000, 0, 0) != 0) { return 26; }
    first = linux_call(9, 0, 5000, 1, 1, fd, 0);
    if (first < 0) { return 27; }
    a = (void *)first;
    if (a[4999] != 0 || a[5000] != 0 || a[8191] != 0) { return 28; }
    if (call(11, first, 8192, 0, 0) != 0 || call(77, fd, 0, 0, 0) != 0) { return 29; }
    call(3, fd, 0, 0, 0);
    call(87, (word)name, 0, 0, 0);
    int result = uncached_private_mapping();
    if (result != 0) { return result; }
    result = append_existing_blocks();
    if (result != 0) { return result; }
    result = directory_with_unrelated_appends();
    if (result != 0) { return result; }
    result = snapshot_write_prepare_and_fixed_reload();
    if (result != 0) { return result; }
    result = readonly_snapshot_tail_and_retirement();
    if (result != 0) { return result; }
    result = private_mapping_and_fork();
    if (result != 0) { return result; }
    result = syscall_read_backing();
    if (result != 0) { return result; }
    result = unix_coalesced_copyout_faults();
    if (result != 0) { return result; }
    result = readlink_kind_cases();
    if (result != 0) { return result; }
    result = rooted_open_cases();
    if (result != 0) { return result; }
    result = root_directory_cursor_cases();
    if (result != 0) { return result; }
    result = unix_stream_append_cases();
    if (result != 0) { return result; }
    static const char pass[] = "FILE-MAPPING-PASS\n";
    call(1, 1, (word)pass, sizeof(pass) - 1, 0);
    return 0;
}
