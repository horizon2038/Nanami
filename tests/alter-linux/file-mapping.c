/* Shared aliases, private initialization and fork of a deleted executable. */
typedef unsigned long word;
extern long linux_call(long, word, word, word, word, word, word);
static long call(long n, word a, word b, word c, word d) {
    return linux_call(n, a, b, c, d, 0, 0);
}
_Alignas(4096) static volatile unsigned char protected_state[4096];
_Alignas(4096) static volatile unsigned char readlink_state[4096];
extern long thread_start(word, word, word, word, word, word);
extern long clone3_start(void *, word, word);
extern long vfork_start(word);
extern const unsigned char exec_stack_probe_start[], exec_stack_probe_end[];
static volatile word spawn_marker;
static word spawn_clear_handlers;
static word spawn_altstack;
static const char *spawn_path;
static int shared_exec_worker(void) {
    const char *argv[] = {spawn_path, 0}; const char *env[] = {0};
    word action[4], mask, altstack[3];
    if (call(13, 10, 0, (word)action, 8) != 0
        || action[0] != (spawn_clear_handlers ? 0 : (word)shared_exec_worker)
        || call(13, 12, 0, (word)action, 8) != 0 || action[0] != 1
        || call(14, 2, 0, (word)&mask, 8) != 0 || mask != (1ul << 9)
        || call(131, 0, (word)altstack, 0, 0) != 0
        || altstack[0] != spawn_altstack || altstack[1] != 0 || altstack[2] != 16384) {
        spawn_marker = 0xbad; return 229;
    }
    spawn_marker = 0x1234;
    long result = call(59, (word)spawn_path, (word)argv, (word)env, 0);
    spawn_marker = result == -2 ? 0xface : 0xbad;
    return result == -2 ? 0 : 229;
}

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
    if (call(61, child, (word)&status, 0, 0) != child || status != 0) { return 230; }
    long stack = linux_call(9, 0, 65536, 3, 0x22, -1ul, 0);
    if (stack < 0) { return 230; }
    word old_caught[4], old_ignored[4], old_mask, old_altstack[3];
    word caught[4] = {(word)shared_exec_worker, 0, 0, 0}, ignored[4] = {1, 0, 0, 0};
    word mask = 1ul << 9;
    spawn_altstack = stack + 16384;
    word altstack[3] = {spawn_altstack, 0, 16384};
    if (call(13, 10, (word)caught, (word)old_caught, 8) != 0
        || call(13, 12, (word)ignored, (word)old_ignored, 8) != 0
        || call(14, 2, (word)&mask, (word)&old_mask, 8) != 0
        || call(131, (word)altstack, (word)old_altstack, 0, 0) != 0) { return 230; }
    for (word attempt = 0; attempt < 18; ++attempt) {
        spawn_marker = 0;
        word mode = (attempt / 2) % 3;
        spawn_clear_handlers = mode == 1;
        spawn_path = attempt & 1 ? path : "/nonexistent-vfork-exec";
        if (mode == 1) {
            word args[11] = {0x4100 | (1ul << 32), 0, 0, 0, 17, (word)stack, 65536, 0, 0, 0, 0};
            child = clone3_start(args, sizeof(args), (word)shared_exec_worker);
        } else if (mode == 0) {
            child = thread_start(0x4100 | 17, stack + 65536, 0, 0, 0, (word)shared_exec_worker);
        } else {
            child = vfork_start((word)shared_exec_worker);
        }
        if (child < 0 || spawn_marker != ((attempt & 1) ? 0x1234ul : 0xfaceul)) { return 230; }
        status = -1;
        if (call(61, child, (word)&status, 0, 0) != child || status != 0
            || *a != 0x6a || *b != 0x7b) { return 230; }
        word action[4];
        if (call(13, 10, 0, (word)action, 8) != 0 || action[0] != caught[0]) { return 230; }
    }
    if (call(13, 10, (word)old_caught, 0, 8) != 0
        || call(13, 12, (word)old_ignored, 0, 8) != 0
        || call(14, 2, (word)&old_mask, 0, 8) != 0
        || call(131, (word)old_altstack, 0, 0, 0) != 0) { return 230; }
    if (call(11, stack, 65536, 0, 0) != 0 || call(87, (word)path, 0, 0, 0) != 0) { return 230; }
    static const char spawn_pass[] = "SHARED-VFORK-EXEC-PASS cases=18\n";
    call(1, 1, (word)spawn_pass, sizeof(spawn_pass) - 1, 0);
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

static int writable_private_fork(void) {
    const word bytes = 64 * 4096;
    long base = linux_call(9, 0, bytes, 3, 0x22, -1UL, 0);
    if (base < 0) { return 160; }
    volatile unsigned char *p = (void *)base;
    for (word page = 0; page < 64; ++page) { p[page * 4096] = (unsigned char)(page + 32); }
    volatile word stack_value = 0x12345678;
    long child = call(57, 0, 0, 0, 0);
    if (child < 0) { return 161; }
    if (child == 0) {
        stack_value = 0xabcdef;
        p[0] = 0xee;
        /* Alpha copyout must detach COW even though logical rights are RW. */
        if (call(118, base + 2 * 4096, base + 3 * 4096, base + 4 * 4096, 0) != 0
            || p[2 * 4096] != 0 || p[3 * 4096] != 0 || p[4 * 4096] != 0) {
            call(60, 162, 0, 0, 0); return 162;
        }
        long grandchild = call(57, 0, 0, 0, 0);
        if (grandchild < 0) { call(60, 163, 0, 0, 0); return 163; }
        if (grandchild == 0) {
            p[5 * 4096] = 0xdd;
            call(60, p[0] == 0xee && stack_value == 0xabcdef ? 0 : 164, 0, 0, 0);
            return 164;
        }
        int status = -1;
        if (call(61, grandchild, (word)&status, 0, 0) != grandchild || status != 0
            || p[5 * 4096] != 37) { call(60, 165, 0, 0, 0); return 165; }
        call(60, 0, 0, 0, 0); return 166;
    }
    int status = -1;
    if (call(61, child, (word)&status, 0, 0) != child || status != 0
        || stack_value != 0x12345678) { return 167; }
    for (word page = 0; page < 64; ++page) {
        if (p[page * 4096] != (unsigned char)(page + 32)) { return 168; }
    }
    /* Last-owner write must restore WRITE without copying stale peer data. */
    p[0] = 0xcc;
    if (p[0] != 0xcc || call(11, base, bytes, 0, 0) != 0) { return 169; }
    return 0;
}

static int memfd_private_fork_runs(void) {
    static unsigned char data[12 * 4096 + 17];
    for (word i = 0; i < sizeof(data); ++i) { data[i] = (unsigned char)(i % 239); }
    long fd = call(319, (word)"fork-runs", 0, 0, 0);
    if (fd < 0 || call(1, fd, (word)data, sizeof(data), 0) != sizeof(data)) { return 150; }
    long base = linux_call(9, 0, 16 * 4096, 3, 2, fd, 0);
    if (base < 0 || call(10, base, 4 * 4096, 1, 0) != 0
        || call(10, base + 4 * 4096, 4 * 4096, 0, 0) != 0) { return 151; }
    long child = call(57, 0, 0, 0, 0);
    if (child < 0) { return 152; }
    if (child == 0) {
        if (call(10, base, 13 * 4096, 3, 0) != 0) { call(60, 153, 0, 0, 0); return 153; }
        volatile unsigned char *p = (void *)base;
        for (word i = 0; i < 13 * 4096; ++i) {
            if (p[i] != (i < sizeof(data) ? data[i] : 0)) { call(60, 154, 0, 0, 0); return 154; }
        }
        p[0] = 0x7f;
        call(60, 0, 0, 0, 0);
        return 155;
    }
    int status = -1;
    if (call(61, child, (word)&status, 0, 0) != child || status != 0
        || call(10, base, 13 * 4096, 3, 0) != 0) { return 156; }
    volatile unsigned char *p = (void *)base;
    for (word i = 0; i < 13 * 4096; ++i) {
        if (p[i] != (i < sizeof(data) ? data[i] : 0)) { return 157; }
    }
    if (call(11, base, 16 * 4096, 0, 0) != 0 || call(3, fd, 0, 0, 0) != 0) { return 158; }
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

static int fixed_writable_reuse_and_reload(void) {
    static unsigned char data[5003];
    for (word i = 0; i < sizeof(data); ++i) { data[i] = (unsigned char)(i % 181 + 1); }
    for (word copied = 0; copied < 2; ++copied) {
        for (word executable = 0; executable < 2; ++executable) {
            const char *name = "/tmp/fixed-writable-reuse";
            long fd = call(2, (word)name, 01102, 0600, 0);
            if (fd < 0 || call(1, fd, (word)data, sizeof(data), 0) != sizeof(data)) { return 239; }
            /* A cold nonzero offset bypasses whole-file cache admission,
             * exercising copied private backing as well as snapshot aliases. */
            word offset = copied ? 4096 : 0;
            long peer = linux_call(9, 0, 8192, 1, 2, fd, offset);
            long address = linux_call(9, 0, 12288, 1, 2, fd, offset);
            if (peer < 0 || address < 0
                || linux_call(9, address, 8192, executable ? 7 : 3, 18, fd, offset) != address) { return 240; }
            volatile unsigned char *p = (void *)address;
            volatile unsigned char *q = (void *)peer;
            for (word i = 0; i < 12288; ++i) {
                if (p[i] != (offset + i < sizeof(data) ? data[offset + i] : 0)) { return 241; }
            }
            p[0] = 0x91; p[4095] = 0x82; p[8191] = 0x73;
            unsigned char disk = 0;
            if (q[0] != data[offset]
                || q[4095] != (offset + 4095 < sizeof(data) ? data[offset + 4095] : 0)
                || q[8191] != 0
                || call(17, fd, (word)&disk, 1, offset) != 1 || disk != data[offset]) { return 242; }
            /* Writable reuse consumes the immutable proof. Returning to READ
             * cannot certify those dirty bytes for a later fixed file mmap. */
            if (call(10, address, 8192, 1, 0) != 0
                || linux_call(9, address, 8192, 1, 18, fd, offset) != address) { return 243; }
            for (word i = 0; i < 12288; ++i) {
                if (p[i] != (offset + i < sizeof(data) ? data[offset + i] : 0)) { return 244; }
            }
            if (call(11, address, 12288, 0, 0) != 0 || call(11, peer, 8192, 0, 0) != 0
                || call(3, fd, 0, 0, 0) != 0 || call(87, (word)name, 0, 0, 0) != 0) { return 245; }
        }
    }
    static const char pass[] = "TINYCORE-FIXED-WRITABLE-REUSE-PASS cases=4\n";
    call(1, 1, (word)pass, sizeof(pass) - 1, 0);
    return 0;
}

static int readonly_fork_tail(void) {
    const word bytes = 512 * 1024;
    long address = linux_call(9, 0, bytes, 3, 0x22, -1ul, 0);
    if (address < 0) { return 231; }
    volatile unsigned char *p = (void *)address;
    for (word i = 0; i < bytes; ++i) { p[i] = (unsigned char)(i % 193 + 1); }
    if (call(10, address, bytes, 1, 0) != 0) { return 232; }
    long child = call(57, 0, 0, 0, 0);
    if (child < 0) { return 233; }
    if (child == 0) {
        /* Upgrade one tail page before any child read can populate the run. */
        word page = address + 64 * 4096;
        if (call(10, page, 4096, 3, 0) != 0) { return 234; }
        p[64 * 4096] = 0xfe;
        for (word i = 16 * 4096; i < bytes; ++i) {
            unsigned char expected = i == 64 * 4096 ? 0xfe : (unsigned char)(i % 193 + 1);
            if (p[i] != expected) { return 235; }
        }
        call(60, 0, 0, 0, 0);
        for (;;) {}
    }
    int status = -1;
    if (call(61, child, (word)&status, 0, 0) != child || status != 0) { return 236; }
    for (word i = 0; i < bytes; ++i) {
        if (p[i] != (unsigned char)(i % 193 + 1)) { return 237; }
    }
    if (call(11, address, bytes, 0, 0) != 0) { return 238; }
    static const char pass[] = "READONLY-FORK-TAIL-PASS\n";
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
    if (address < 0 || ((word)address & ((1ul<<21)-1)) != 0) { return 54; }
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
    if (address < 0 || ((word)address & ((1ul<<21)-1)) != 0) { return 58; }
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
static int disjoint_mmap_view_cases(void) {
    long base = linux_call(9, 0, 8192, 3, 0x22, -1, 0);
    int pipefd[2];
    if (base < 0 || call(22, (word)pipefd, 0, 0, 0) != 0) { return 160; }
    volatile unsigned char *source = (void *)base;
    volatile unsigned char *output = (void *)(base + 4096);
    long mappings[8];
    for (word index = 0; index < 8; ++index) {
        /* Keep unrelated mappings live: unmap deliberately retains its old
         * conservative generation invalidation. Both pipe transfers exercise
         * ordinary checked read/write views into the original two pages. */
        for (word byte = 0; byte < 64; ++byte) { source[byte] = (unsigned char)(index + byte); }
        if (call(1, pipefd[1], base, 64, 0) != 64
            || call(0, pipefd[0], base + 4096, 64, 0) != 64) { return 161; }
        for (word byte = 0; byte < 64; ++byte) {
            if (output[byte] != (unsigned char)(index + byte)) { return 162; }
        }
        mappings[index] = linux_call(9, 0, 4096, 3, 0x22, -1, 0);
        if (mappings[index] < 0) { return 163; }
    }
    if (linux_call(9, base, 4096, 3, 0x32, -1, 0) != base) { return 164; }
    for (word byte = 0; byte < 64; ++byte) { source[byte] = (unsigned char)(0x90 + byte); }
    if (linux_call(9, base + 4096, 4096, 3, 0x32, -1, 0) != base + 4096) { return 165; }
    if (call(1, pipefd[1], base, 64, 0) != 64
        || call(0, pipefd[0], base + 4096, 64, 0) != 64) { return 166; }
    for (word byte = 0; byte < 64; ++byte) {
        if (output[byte] != (unsigned char)(0x90 + byte)) { return 167; }
    }
    call(3, pipefd[0], 0, 0, 0); call(3, pipefd[1], 0, 0, 0);
    for (word index = 0; index < 8; ++index) {
        if (call(11, mappings[index], 4096, 0, 0) != 0) { return 168; }
    }
    if (call(11, base, 8192, 0, 0) != 0) { return 169; }
    return 0;
}

static int large_copyout_view_cases(void) {
    static unsigned char data[65536];
    const char *name = "/tmp/large-copyout-view-test";
    const word mapping_bytes = 64 * 4096;
    long address = linux_call(9, 0, mapping_bytes, 3, 0x22, -1, 0);
    long fd = call(2, (word)name, 0102, 0600, 0);
    if (address < 0 || fd < 0) { return 233; }
    volatile unsigned char *mapping = (void *)address;
    const word start = 2 * 4096 + 19;
    for (word i = 0; i < mapping_bytes; ++i) { mapping[i] = 0x55; }
    for (word epoch = 0; epoch < 3; ++epoch) {
        for (word i = 0; i < sizeof(data); ++i) { data[i] = (unsigned char)(i * 37 + epoch * 11); }
        if (call(18, fd, (word)data, sizeof(data), 0) != sizeof(data)
            || call(17, fd, address + start, sizeof(data), 0) != sizeof(data)) { return 234; }
        for (word i = 0; i < mapping_bytes; ++i) {
            unsigned char expected = i >= start && i < start + sizeof(data) ? data[i - start] : 0x55;
            if (mapping[i] != expected) { return 235; }
        }
    }
    /* The parent keeps epoch2. A private child replaces a middle page and
     * reads epoch3 across17 pages, including the remaining COW pages. */
    for (word i = 0; i < sizeof(data); ++i) { data[i] = (unsigned char)(i * 37 + 33); }
    if (call(18, fd, (word)data, sizeof(data), 0) != sizeof(data)) { return 236; }
    long child = call(57, 0, 0, 0, 0);
    if (child < 0) { return 237; }
    if (child == 0) {
        if (linux_call(9, address + 10 * 4096, 4096, 3, 0x32, -1, 0) != address + 10 * 4096
            || call(17, fd, address + start, sizeof(data), 0) != sizeof(data)) {
            call(60, 238, 0, 0, 0); for (;;) {}
        }
        for (word i = 0; i < mapping_bytes; ++i) {
            unsigned char expected = i >= start && i < start + sizeof(data) ? data[i - start] : 0x55;
            if (mapping[i] != expected) { call(60, 239, 0, 0, 0); for (;;) {} }
        }
        call(60, 0, 0, 0, 0); for (;;) {}
    }
    int status = 0;
    if (call(61, child, (word)&status, 0, 0) != child || status != 0) { return 240; }
    for (word i = 0; i < mapping_bytes; ++i) {
        unsigned char expected = i >= start && i < start + sizeof(data)
            ? (unsigned char)((i - start) * 37 + 22) : 0x55;
        if (mapping[i] != expected) { return 241; }
    }
    if (call(3, fd, 0, 0, 0) != 0 || call(87, (word)name, 0, 0, 0) != 0
        || call(11, address, mapping_bytes, 0, 0) != 0) { return 242; }
    return 0;
}

static volatile word huge_running, huge_stop;
static unsigned char huge_pattern(word i) {return (unsigned char)(i*17+(i>>12)*31+0x37);}
static int huge_mremap_copy_cases(void) {
    const word source=0x30000000, target=0x30800000, bytes=4*1024*1024;
    if (linux_call(9,source,bytes,3,0x32,-1ul,0)!=(long)source) {return 243;}
    volatile unsigned char *p=(void *)source;
    for (word i=0;i<bytes;++i) {p[i]=huge_pattern(i);}
    if (linux_call(25,source,bytes,bytes,3,target,0)!=(long)target) {return 243;}
    p=(void *)target;
    for (word i=0;i<bytes;++i) {if (p[i]!=huge_pattern(i)) {return 243;}}
    /* Reusing the released source must not alias or overwrite the result. */
    if (linux_call(9,source,bytes,3,0x32,-1ul,0)!=(long)source) {return 243;}
    volatile unsigned char *q=(void *)source;
    for (word i=0;i<bytes;++i) {if (q[i]!=0) {return 243;} q[i]=0xa5;}
    if (call(10,target+4096,4096,1,0)!=0
        || call(10,target+4096,4096,3,0)!=0) {return 243;}
    for (word i=0;i<bytes;++i) {if (p[i]!=huge_pattern(i)) {return 243;}}
    if (call(11,source,bytes,0,0)!=0 || call(11,target,bytes,0,0)!=0) {return 243;}
    static const char pass[]="TINYCORE-HUGE-COPY-PASS\n";
    call(1,1,(word)pass,sizeof(pass)-1,0);
    return 0;
}
static int huge_neighbor_worker(void) {
    volatile unsigned char *neighbor=(void *)0x28008000;
    __atomic_store_n(&huge_running,1,__ATOMIC_RELEASE);
    while (!__atomic_load_n(&huge_stop,__ATOMIC_ACQUIRE)) {
        if (*neighbor != huge_pattern(0x8000)) {return 243;}
        call(24,0,0,0,0);
    }
    return *neighbor == huge_pattern(0x8000) ? 0 : 243;
}

static int huge_after_small_table_cases(void) {
    const word base=0x2c000000, large=2*1024*1024, bytes=4*large;
    /* Leave empty 4K tables in the first and third intervals. The second
       and fourth still have room for real huge leaves. */
    for (word offset=0;offset<bytes;offset+=2*large) {
        if (linux_call(9,base+offset,4096,3,0x32,-1ul,0)!=(long)(base+offset)
            || call(11,base+offset,4096,0,0)!=0) {return 253;}
    }
    if (linux_call(9,base,bytes,3,0x32,-1ul,0)!=base) {return 253;}
    volatile unsigned char *p=(void *)base;
    for (word i=0;i<bytes;++i) {
        if (p[i]!=0) {return 254;}
        p[i]=huge_pattern(i);
    }
    int pipefd[2];
    if (call(22,(word)pipefd,0,0,0)!=0
        || call(1,pipefd[1],base+large-16,32,0)!=32
        || call(0,pipefd[0],base+3*large-16,32,0)!=32) {return 254;}
    call(3,pipefd[0],0,0,0);
    call(3,pipefd[1],0,0,0);
    if (call(10,base+3*large+4096,4096,1,0)!=0
        || call(10,base+3*large+4096,4096,3,0)!=0) {return 254;}
    for (word i=0;i<bytes;++i) {
        word source=(i>=3*large-16 && i<3*large+16)?i-2*large:i;
        if (p[i]!=huge_pattern(source)) {return 254;}
    }
    if (call(11,base,bytes,0,0)!=0) {return 254;}
    static const char pass[]="GUEST-HUGE-AFTER-SMALL-TABLE-PASS\n";
    call(1,1,(word)pass,sizeof(pass)-1,0);
    return 0;
}

static int private_copied_fork_cases(void) {
    const word bases[]={0x34001000,0x34400000,0x35001000,0x34800000,0x35401000,0x34c00000};
    const word sizes[]={4*4096,2*1024*1024,4*4096,2*1024*1024,4*4096,2*1024*1024};
    const word rights[]={0,0,2,2,6,6};
    for (word run=0;run<6;++run) {
        word base=bases[run],bytes=sizes[run];
        if (linux_call(9,base,bytes,3,0x32,-1ul,0)!=(long)base) {return 255;}
        volatile unsigned char *p=(void *)base;
        for (word i=0;i<bytes;++i) {p[i]=huge_pattern(i);}
        if (call(10,base,bytes,rights[run],0)!=0) {return 255;}
        long child=call(57,0,0,0,0);
        if (child<0) {return 255;}
        if (child==0) {
            /* This inaccessible huge copy had no guest page tables. */
            if (run==1 && (call(10,base+4096,4096,3,0)!=0 || p[4096]!=huge_pattern(4096))) {
                call(60,255,0,0,0);for (;;) {}
            }
            int bad=call(10,base,bytes,3,0)!=0;
            if (!bad) {
                for (word i=0;i<bytes;++i) {if (p[i]!=huge_pattern(i)) {bad=1;break;}}
                p[0]^=0xff;
            }
            call(60,bad?255:0,0,0,0);for (;;) {}
        }
        int status=-1;
        if (call(61,child,(word)&status,0,0)!=child || status!=0
            || call(10,base,bytes,3,0)!=0) {return 255;}
        for (word i=0;i<bytes;++i) {if (p[i]!=huge_pattern(i)) {return 255;}}
        if (call(11,base,bytes,0,0)!=0) {return 255;}
    }
    static const char pass[]="COPIED-PRIVATE-FORK-PASS\n";
    call(1,1,(word)pass,sizeof(pass)-1,0);
    return 0;
}

static int huge_private_cases(void) {
    const word mixed_base=0x35c00000;
    const word mixed_boundary=2*1024*1024;
    const word mixed_bytes=2*mixed_boundary+4096;
    if (linux_call(9,mixed_base,mixed_bytes,3,0x32,-1ul,0)!=(long)mixed_base) {return 255;}
    volatile unsigned char *mixed=(void *)mixed_base;
    mixed[0]=0x35;
    mixed[mixed_boundary]=0x29;
    mixed[mixed_bytes-1]=0x67;
    if (call(10,mixed_base,mixed_boundary,1,0)!=0
        || call(10,mixed_base,mixed_bytes,0,0)!=0
        || call(10,mixed_base,mixed_bytes,3,0)!=0
        || call(10,mixed_base+4096,mixed_bytes-4096,1,0)!=0
        || mixed[0]!=0x35 || mixed[mixed_boundary]!=0x29 || mixed[mixed_bytes-1]!=0x67) {return 255;}
    mixed[0]=0x53;
    if (call(10,mixed_base+4096,mixed_bytes-4096,0,0)!=0
        || call(10,mixed_base+4096,mixed_bytes-4096,3,0)!=0
        || mixed[0]!=0x53 || mixed[mixed_boundary]!=0x29 || mixed[mixed_bytes-1]!=0x67
        || call(11,mixed_base,mixed_bytes,0,0)!=0) {return 255;}
    /* An automatic large request after a small one must still have a complete
       huge interval. Padding reserves neither physical RAM nor guest leaves. */
    long small=linux_call(9,0,4096,3,0x22,-1ul,0);
    long automatic=linux_call(9,0,2*1024*1024,3,0x22,-1ul,0);
    if (small<0 || automatic<0 || ((word)automatic&((1ul<<21)-1))!=0) {return 255;}
    volatile unsigned char *auto_page=(void *)automatic;
    for (word i=0;i<2*1024*1024;++i) {
        if (auto_page[i]!=0) {return 255;}
        auto_page[i]=huge_pattern(i);
    }
    for (word i=0;i<2*1024*1024;++i) {
        if (auto_page[i]!=huge_pattern(i)) {return 255;}
    }
    if (call(10,automatic,2*1024*1024,1,0)!=0
        || call(10,automatic,2*1024*1024,0,0)!=0
        || call(10,automatic,2*1024*1024,3,0)!=0) {return 255;}
    for (word i=0;i<2*1024*1024;++i) {
        if (auto_page[i]!=huge_pattern(i)) {return 255;}
    }
    if (call(11,automatic,2*1024*1024,0,0)!=0 || call(11,small,4096,0,0)!=0) {return 255;}
    const word base=0x28000000, bytes=4*1024*1024;
    if (linux_call(9,base,bytes,3,0x32,-1ul,0) != base) {return 243;}
    volatile unsigned char *p=(void *)base;
    for (word i=0;i<bytes;++i) {if (p[i]!=0) {return 244;} p[i]=huge_pattern(i);}
    if (call(10,base,bytes,1,0)!=0 || call(10,base,bytes,3,0)!=0) {return 244;}
    /* Copyout must address a 4K slice inside the huge Frame, including a
       boundary crossing, without exporting a whole huge capability as 4K. */
    int pipefd[2];
    if (call(22,(word)pipefd,0,0,0)!=0
        || call(1,pipefd[1],base+0x1ffff0,32,0)!=32
        || call(0,pipefd[0],base+0x200ff0,32,0)!=32) {return 245;}
    for (word i=0;i<32;++i) {if (p[0x200ff0+i]!=huge_pattern(0x1ffff0+i)) {return 245;}}
    call(3,pipefd[0],0,0,0);call(3,pipefd[1],0,0,0);
    long stack=linux_call(9,0,65536,3,0x22,-1ul,0);
    if (stack<0) {return 246;}
    huge_running=huge_stop=0;
    long peer=thread_start(0x100|17,stack+65536,0,0,0,(word)huge_neighbor_worker);
    if (peer<0) {return 246;}
    while (!__atomic_load_n(&huge_running,__ATOMIC_ACQUIRE)) {call(24,0,0,0,0);}
    int bad=call(10,base+4096,4096,1,0)!=0 || p[4096]!=huge_pattern(4096)
        || call(10,base+4096,4096,3,0)!=0;
    __atomic_store_n(&huge_stop,1,__ATOMIC_RELEASE);
    int status=-1;
    if (call(61,peer,(word)&status,0,0)!=peer || status!=0 || bad
        || call(11,stack,65536,0,0)!=0) {return 247;}
    if (call(11,base+0x100000,4096,0,0)!=0
        || linux_call(9,base+0x100000,4096,3,0x32,-1ul,0)!=base+0x100000) {return 248;}
    for (word i=0;i<4096;++i) {if (p[0x100000+i]!=0) {return 248;}}
    /* The second huge leaf remains intact until private fork. */
    long child=call(57,0,0,0,0);
    if (child<0) {return 249;}
    if (child==0) {
        p[0x300003]=0xa4;
        call(60,p[0x300004]==huge_pattern(0x300004)?0:250,0,0,0);
        for (;;) {}
    }
    status=-1;
    if (call(61,child,(word)&status,0,0)!=child || status!=0
        || p[0x300003]!=huge_pattern(0x300003)) {return 250;}
    for (word i=0;i<bytes;++i) {
        unsigned char expected=(i>=0x100000 && i<0x101000)?0:huge_pattern(i);
        if (i>=0x200ff0 && i<0x201010) {expected=huge_pattern(i-0x200ff0+0x1ffff0);}
        if (p[i]!=expected) {return 251;}
    }
    if (call(11,base,bytes,0,0)!=0) {return 252;}
    /* Whole-leaf release must not build 512 replacement PTEs. */
    if (linux_call(9,base+bytes,2*1024*1024,3,0x32,-1ul,0)!=base+bytes
        || call(11,base+bytes,2*1024*1024,0,0)!=0) {return 252;}
    static const char pass[]="GUEST-HUGE-LIFECYCLE-PASS\n";
    call(1,1,(word)pass,sizeof(pass)-1,0);
    return huge_after_small_table_cases();
}


/* A valid DSO exercises the deferred header/LOAD/fault path, including an
 * untouched suffix in the parent while fork maps the child's aliases. */
static int deferred_elf_snapshot_cases(void) {
    static unsigned char image[512*1024];
    word bytes=sizeof(image);
    /* A nonloaded footer makes this partial-file mapping bypass cache fill. */
    word mapped_bytes=256*1024;
    for (word i=0;i<bytes;++i) {image[i]=(unsigned char)(i/4096+0x30);}
    for (word i=0;i<120;++i) {image[i]=0;}
    image[0]=127;image[1]='E';image[2]='L';image[3]='F';image[4]=2;image[5]=1;image[6]=1;
    elf_word(image+16,3,2);elf_word(image+18,62,2);elf_word(image+20,1,4);
    elf_word(image+24,120,8);elf_word(image+32,64,8);
    elf_word(image+52,64,2);elf_word(image+54,56,2);elf_word(image+56,1,2);
    elf_word(image+64,1,4);elf_word(image+68,5,4);
    elf_word(image+96,mapped_bytes,8);elf_word(image+104,mapped_bytes,8);elf_word(image+112,4096,8);
    const char *path="/tmp/deferred-dso";
    long fd=call(2,(word)path,01102,0600,0);
    if (fd<0) {return 253;}
    for (word i=0;i<bytes;i+=65536) {
        if (call(1,fd,(word)image+i,65536,0)!=65536) {return 253;}
    }
    long base=linux_call(9,0,mapped_bytes,1,2,fd,0);
    if (base<0) {return 253;}
    volatile unsigned char *p=(void *)base;
    if (p[0]!=127) {return 253;}
    /* Reusing a segment with unchanged R rights must still populate it. */
    if (linux_call(9,base+32*4096,8*4096,1,18,fd,32*4096)!=base+32*4096
        || p[35*4096]!=0x53) {return 253;}
    /* A directly initialized absent page already has unique private backing;
     * mprotect must install its final rights without a source owner or copy. */
    if (call(10,base+60*4096,4096,3,0)!=0) {return 253;}
    p[60*4096]=0xee;
    long child=call(57,0,0,0,0);int status=-1;
    if (child<0) {return 253;}
    if (child==0) {
        if (p[40*4096]!=0x58 || call(10,base+40*4096,4096,3,0)!=0) {return 253;}
        p[40*4096]=0xee;call(60,0,0,0,0);for (;;) {}
    }
    if (call(61,child,(word)&status,0,0)!=child || status!=0 || p[40*4096]!=0x58 || p[60*4096]!=0xee) {return 253;}
    if (call(10,base+48*4096,4096,0,0)!=0) {return 253;}
    child=call(57,0,0,0,0);status=-1;
    if (child<0) {return 253;}
    if (child==0) {volatile unsigned char ignored=p[48*4096];(void)ignored;call(60,1,0,0,0);for (;;) {}}
    if (call(61,child,(word)&status,0,0)!=child || (status&127)!=11
        || call(10,base+48*4096,4096,1,0)!=0 || p[48*4096]!=0x60) {return 253;}
    if (call(11,base+56*4096,4096,0,0)!=0
        || linux_call(9,base+56*4096,4096,3,0x32,-1ul,0)!=base+56*4096
        || p[56*4096]!=0 || p[55*4096]!=0x67 || p[57*4096]!=0x69) {return 253;}
    if (call(11,base,mapped_bytes,0,0)!=0) {return 253;}
    base=linux_call(9,0,mapped_bytes,1,2,fd,0);
    if (base<0 || call(11,base,mapped_bytes,0,0)!=0 || call(3,fd,0,0,0)!=0
        || call(87,(word)path,0,0,0)!=0) {return 253;}
    static const char pass[]="DEFERRED-ELF-SNAPSHOT-PASS\n";
    call(1,1,(word)pass,sizeof(pass)-1,0);return 0;
}

static int writable_fork_prefix(void) {
    const word bytes = 640 * 1024;
    long address = linux_call(9, 0, bytes, 3, 0x22, -1ul, 0);
    if (address < 0) { return 239; }
    volatile unsigned char *p = (void *)address;
    for (word i = 0; i < bytes; ++i) { p[i] = (unsigned char)(i % 193 + 1); }
    long child = call(57, 0, 0, 0, 0);
    if (child < 0) { return 240; }
    if (child == 0) {
        const word offsets[] = {0x12000, 0x25000, 0x42000, 0x7f000};
        for (word i = 0; i < sizeof(offsets) / sizeof(offsets[0]); ++i) {
            word offset = offsets[i];
            if (p[offset] != (unsigned char)(offset % 193 + 1)) { return 241; }
            p[offset] = 0xfe;
            if (p[offset] != 0xfe) { return 242; }
        }
        call(60, 0, 0, 0, 0);
        for (;;) {}
    }
    int status = -1;
    if (call(61, child, (word)&status, 0, 0) != child || status != 0) { return 243; }
    for (word i = 0; i < bytes; ++i) {
        if (p[i] != (unsigned char)(i % 193 + 1)) { return 244; }
    }
    /* A surviving parent must also regain WRITE after the child releases COW. */
    p[0x42000] = 0xab;
    if (p[0x42000] != 0xab || call(11, address, bytes, 0, 0) != 0) { return 245; }
    static const char pass[] = "FORK-COW-PREFIX-PASS\n";
    call(1, 1, (word)pass, sizeof(pass) - 1, 0);
    return 0;
}

static volatile word zero_alias_ready, zero_alias_stop, zero_alias_bad;
static int zero_alias_reader(void) {
    volatile unsigned char *p = (void *)0x2a010000;
    if (p[65 * 4096] != 0) { __atomic_store_n(&zero_alias_bad, 1, __ATOMIC_RELAXED); }
    __atomic_store_n(&zero_alias_ready, 1, __ATOMIC_RELEASE);
    while (!__atomic_load_n(&zero_alias_stop, __ATOMIC_ACQUIRE)) {
        if (p[65 * 4096] != 0) { __atomic_store_n(&zero_alias_bad, 1, __ATOMIC_RELAXED); }
        call(24, 0, 0, 0, 0);
    }
    return __atomic_load_n(&zero_alias_bad, __ATOMIC_ACQUIRE) ? 227 : 0;
}

static int shared_readonly_zero_cases(void) {
    const word base = 0x2a010000, page = 4096, bytes = 129 * page;
    /* Fixed and below2MiB: exercise the small-page zero-alias fallback before
       fork/snapshot flags exist. Reads must map several VAs of the same Frame. */
    if (linux_call(9, base, bytes, 1, 0x32, -1ul, 0) != (long)base) { return 224; }
    volatile unsigned char *p = (void *)base;
    if (p[0] || p[page] || p[128 * page]) { return 225; }
    long stack = linux_call(9, 0, 65536, 3, 0x22, -1ul, 0);
    if (stack < 0) { return 226; }
    __atomic_store_n(&zero_alias_ready, 0, __ATOMIC_RELAXED);
    __atomic_store_n(&zero_alias_stop, 0, __ATOMIC_RELAXED);
    __atomic_store_n(&zero_alias_bad, 0, __ATOMIC_RELAXED);
    long peer = thread_start(0x100 | 17, stack + 65536, 0, 0, 0, (word)zero_alias_reader);
    if (peer < 0) { return 226; }
    while (!__atomic_load_n(&zero_alias_ready, __ATOMIC_ACQUIRE)) { call(24, 0, 0, 0, 0); }
    int failed = call(10, base + 64 * page, page, 3, 0) != 0;
    if (!failed) { p[64 * page] = 0xa7; failed = p[65 * page] != 0; }
    __atomic_store_n(&zero_alias_stop, 1, __ATOMIC_RELEASE);
    int status = -1;
    if (call(61, peer, (word)&status, 0, 0) != peer || status != 0
        || __atomic_load_n(&zero_alias_bad, __ATOMIC_ACQUIRE)
        || call(11, stack, 65536, 0, 0) != 0 || failed) { return 227; }
    /* Retained PROT_NONE backing also needs COW before a WRITE upgrade. */
    if (call(10, base + 128 * page, page, 0, 0) != 0
        || call(10, base + 128 * page, page, 3, 0) != 0) { return 228; }
    p[128 * page] = 0xb8;
    if (p[0] || p[65 * page] || p[127 * page] || p[64 * page] != 0xa7) { return 228; }
    /* A deferred alias must not be reused as unique RAM while siblings live. */
    if (call(11, base, 64 * page, 0, 0) != 0
        || linux_call(9, base, page, 3, 0x32, -1ul, 0) != (long)base) { return 229; }
    p[0] = 0x99;
    int pipefd[2]; unsigned char value = 0xcc;
    if (call(22, (word)pipefd, 0, 0, 0) != 0
        || call(1, pipefd[1], (word)&value, 1, 0) != 1
        || call(0, pipefd[0], base + 64 * page + 1, 1, 0) != 1
        || call(3, pipefd[0], 0, 0, 0) != 0 || call(3, pipefd[1], 0, 0, 0) != 0
        || p[65 * page] || p[127 * page] || p[64 * page] != 0xa7
        || p[64 * page + 1] != 0xcc || p[128 * page] != 0xb8) { return 229; }
    if (call(11, base + 64 * page, 65 * page, 0, 0) != 0
        || call(11, base, page, 0, 0) != 0) { return 230; }
    static const char pass[] = "SHARED-READONLY-ZERO-PASS\n";
    call(1, 1, (word)pass, sizeof(pass) - 1, 0);
    return 0;
}

static volatile word writable_zero_ready, writable_zero_stop, writable_zero_bad;
static int writable_zero_reader(void) {
    volatile unsigned char *p = (void *)0x2b010000;
    __atomic_store_n(&writable_zero_ready, 1, __ATOMIC_RELEASE);
    while (!__atomic_load_n(&writable_zero_stop, __ATOMIC_ACQUIRE)) {
        if (p[65 * 4096] || p[127 * 4096] || p[199 * 4096]) {
            __atomic_store_n(&writable_zero_bad, 1, __ATOMIC_RELEASE); break;
        }
        call(24, 0, 0, 0, 0);
    }
    return 0;
}

static int writable_zero_cases(void) {
    const word base = 0x2b010000, page = 4096, bytes = 256 * page;
    if (linux_call(9, base, bytes, 3, 0x32, -1ul, 0) != (long)base) { return 231; }
    volatile unsigned char *p = (void *)base;
    for (word i = 0; i < 256; ++i) { if (p[i * page]) { return 232; } }
    long stack = linux_call(9, 0, 65536, 3, 0x22, -1ul, 0);
    if (stack < 0) { return 233; }
    __atomic_store_n(&writable_zero_ready, 0, __ATOMIC_RELAXED);
    __atomic_store_n(&writable_zero_stop, 0, __ATOMIC_RELAXED);
    __atomic_store_n(&writable_zero_bad, 0, __ATOMIC_RELAXED);
    long peer = thread_start(0x100 | 17, stack + 65536, 0, 0, 0, (word)writable_zero_reader);
    if (peer < 0) { return 233; }
    while (!__atomic_load_n(&writable_zero_ready, __ATOMIC_ACQUIRE)) { call(24, 0, 0, 0, 0); }
    p[64 * page] = 0xa7;
    int pipefd[2]; unsigned char value = 0xcc;
    int failed = call(22, (word)pipefd, 0, 0, 0) != 0;
    if (!failed) {
        failed = call(1, pipefd[1], (word)&value, 1, 0) != 1
            || call(0, pipefd[0], base + 128 * page + 1, 1, 0) != 1;
        call(3, pipefd[0], 0, 0, 0); call(3, pipefd[1], 0, 0, 0);
    }
    __atomic_store_n(&writable_zero_stop, 1, __ATOMIC_RELEASE);
    int status = -1;
    if (call(61, peer, (word)&status, 0, 0) != peer || status != 0
        || __atomic_load_n(&writable_zero_bad, __ATOMIC_ACQUIRE)
        || call(11, stack, 65536, 0, 0) != 0 || failed) { return 234; }
    long child = call(57, 0, 0, 0, 0);
    if (child == 0) {
        p[200 * page] = 0x71;
        call(60, p[64 * page] == 0xa7 && p[128 * page + 1] == 0xcc && !p[201 * page] ? 0 : 235, 0, 0, 0);
        return 235;
    }
    if (child < 0 || call(61, child, (word)&status, 0, 0) != child || status != 0
        || p[200 * page] || p[64 * page] != 0xa7 || p[128 * page + 1] != 0xcc) { return 235; }
    if (call(11, base, 64 * page, 0, 0) != 0
        || linux_call(9, base, page, 3, 0x32, -1ul, 0) != (long)base
        || call(10, base + 200 * page, page, 0, 0) != 0
        || call(10, base + 200 * page, page, 3, 0) != 0) { return 236; }
    p[0] = 0x99; p[200 * page] = 0x82;
    if (p[65 * page] || p[127 * page] || p[199 * page] || p[201 * page]
        || p[64 * page] != 0xa7 || p[128 * page + 1] != 0xcc
        || call(11, base + 64 * page, 192 * page, 0, 0) != 0
        || call(11, base, page, 0, 0) != 0) { return 237; }
    static const char pass[] = "SHARED-WRITABLE-ZERO-PASS\n";
    call(1, 1, (word)pass, sizeof(pass) - 1, 0);
    return 0;
}

int test_main(void) {
    int writable_zero_result = writable_zero_cases();
    if (writable_zero_result != 0) { return writable_zero_result; }
    int zero_result = shared_readonly_zero_cases();
    if (zero_result != 0) { return zero_result; }
    int prefix_result = writable_fork_prefix();
    if (prefix_result != 0) { return prefix_result; }
    int huge_copy_result=huge_mremap_copy_cases();
    if (huge_copy_result!=0) {return huge_copy_result;}
    int deferred_result=deferred_elf_snapshot_cases();
    if (deferred_result!=0) {return deferred_result;}
    int huge_result=huge_private_cases();
    if (huge_result!=0) {return huge_result;}
    int copied_fork_result=private_copied_fork_cases();
    if (copied_fork_result!=0) {return copied_fork_result;}
    int large_views_result = large_copyout_view_cases();
    if (large_views_result != 0) { return large_views_result; }
    int views_result = disjoint_mmap_view_cases();
    if (views_result != 0) { return views_result; }
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
    result = fixed_writable_reuse_and_reload();
    if (result != 0) { return result; }
    result = readonly_fork_tail();
    if (result != 0) { return result; }
    result = readonly_snapshot_tail_and_retirement();
    if (result != 0) { return result; }
    result = writable_private_fork();
    if (result != 0) { return result; }
    result = memfd_private_fork_runs();
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
