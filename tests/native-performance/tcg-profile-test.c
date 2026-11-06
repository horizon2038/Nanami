/* Compile against the real installed QEMU header; model only its callbacks. */
#if defined(__APPLE__)
#define _DARWIN_C_SOURCE
#endif
#include "tcg-profile.c"
#include <assert.h>

struct qemu_plugin_scoreboard {
    size_t bytes;
    unsigned char *cpu[4];
};
struct qemu_plugin_insn {
    uint64_t physical;
    unsigned char bytes[3];
};
struct qemu_plugin_tb {
    uint64_t pc;
    size_t count, operations;
    struct qemu_plugin_insn insns[3];
    qemu_plugin_u64 entries[3];
    uint64_t additions[3];
};
struct qemu_plugin_scoreboard *qemu_plugin_scoreboard_new(size_t bytes) {
    struct qemu_plugin_scoreboard *score = calloc(1, sizeof(*score));
    assert(score != NULL);
    score->bytes = bytes;
    for (size_t i = 0; i < 4; ++i) {
        score->cpu[i] = calloc(1, bytes);
        assert(score->cpu[i] != NULL);
    }
    return score;
}
void qemu_plugin_scoreboard_free(struct qemu_plugin_scoreboard *score) {
    for (size_t i = 0; i < 4; ++i) {
        free(score->cpu[i]);
    }
    free(score);
}
uint64_t qemu_plugin_u64_get(qemu_plugin_u64 entry, unsigned int cpu) {
    assert(cpu < 4 && entry.offset + sizeof(uint64_t) <= entry.score->bytes);
    uint64_t value;
    memcpy(&value, entry.score->cpu[cpu] + entry.offset, sizeof(value));
    return value;
}
static void set_counter(size_t slot, unsigned int cpu, uint64_t value) {
    qemu_plugin_u64 entry = counter(slot);
    memcpy(entry.score->cpu[cpu] + entry.offset, &value, sizeof(value));
}
int qemu_plugin_num_vcpus(void) { return 4; }
size_t qemu_plugin_tb_n_insns(const struct qemu_plugin_tb *tb) { return tb->count; }
uint64_t qemu_plugin_tb_vaddr(const struct qemu_plugin_tb *tb) { return tb->pc; }
struct qemu_plugin_insn *qemu_plugin_tb_get_insn(const struct qemu_plugin_tb *tb, size_t index) {
    assert(index < tb->count);
    return (struct qemu_plugin_insn *)&tb->insns[index];
}
void *qemu_plugin_insn_haddr(const struct qemu_plugin_insn *insn) {
    return (void *)(uintptr_t)insn->physical;
}
size_t qemu_plugin_insn_data(const struct qemu_plugin_insn *insn, void *buffer, size_t capacity) {
    size_t bytes = MIN(capacity, sizeof(insn->bytes));
    memcpy(buffer, insn->bytes, bytes);
    return bytes;
}
void qemu_plugin_register_vcpu_tb_exec_inline_per_vcpu(struct qemu_plugin_tb *tb,
        enum qemu_plugin_op op, qemu_plugin_u64 entry, uint64_t addition) {
    assert(op == QEMU_PLUGIN_INLINE_ADD_U64 && tb->operations < 3);
    tb->entries[tb->operations] = entry;
    tb->additions[tb->operations++] = addition;
}
void qemu_plugin_register_vcpu_tb_trans_cb(qemu_plugin_id_t id,
        qemu_plugin_vcpu_tb_trans_cb_t cb, void *unused) {
    (void)id; (void)unused;
    assert(cb == translate);
}
void qemu_plugin_register_atexit_cb(qemu_plugin_id_t id,
        qemu_plugin_udata_cb_t cb, void *unused) {
    (void)id; (void)unused;
    assert(cb == finish);
}
static struct qemu_plugin_tb block(uint64_t pc, uint64_t physical, unsigned char byte) {
    struct qemu_plugin_tb tb = {0};
    tb.pc = pc;
    tb.count = 2;
    for (size_t i = 0; i < tb.count; ++i) {
        tb.insns[i].physical = physical + i;
        tb.insns[i].bytes[0] = byte;
        tb.insns[i].bytes[1] = (unsigned char)i;
    }
    translate(&tb, NULL);
    return tb;
}
static void execute(struct qemu_plugin_tb *tb, unsigned int cpu, uint64_t times) {
    for (size_t i = 0; i < tb->operations; ++i) {
        qemu_plugin_u64 entry = tb->entries[i];
        uint64_t value = qemu_plugin_u64_get(entry, cpu) + times * tb->additions[i];
        memcpy(entry.score->cpu[cpu] + entry.offset, &value, sizeof(value));
    }
}
static void setup(size_t capacity, size_t rows) {
    memset(&profile, 0, sizeof(profile));
    g_mutex_init(&profile.mutex);
    profile.capacity = capacity;
    profile.rows = rows;
    profile.max_cpus = 4;
    profile.listener = -1;
    profile.blocks = g_new0(struct block, capacity);
    profile.index = g_hash_table_new(key_hash, key_equal);
    profile.scores = qemu_plugin_scoreboard_new((capacity + 2) * sizeof(uint64_t));
    profile.output = tmpfile();
    assert(profile.output != NULL);
}
static void teardown(void) {
    fclose(profile.output);
    g_hash_table_destroy(profile.index);
    g_free(profile.blocks);
    qemu_plugin_scoreboard_free(profile.scores);
    g_mutex_clear(&profile.mutex);
}
static void output_contains(const char *needle) {
    assert(fflush(profile.output) == 0);
    rewind(profile.output);
    char text[8192] = {0};
    assert(fread(text, 1, sizeof(text) - 1, profile.output) < sizeof(text) - 1);
    assert(strstr(text, needle) != NULL);
    assert(fseek(profile.output, 0, SEEK_END) == 0);
}
static void identity_and_phase(void) {
    setup(8, 8);
    struct qemu_plugin_tb a = block(0x1000, 0x2000, 0x90);
    struct qemu_plugin_tb same = block(0x1000, 0x2000, 0x90);
    struct qemu_plugin_tb other_domain = block(0x1000, 0x3000, 0x90);
    struct qemu_plugin_tb changed = block(0x1000, 0x2000, 0x91);
    assert(profile.used == 3);
    execute(&a, 0, 100);
    execute(&other_domain, 1, 50);
    assert(strcmp(command("end\n"), "ERR invalid-state-or-command\n") == 0);
    assert(strcmp(command("begin\n"), "OK begin\n") == 0);
    assert(strcmp(command("begin\n"), "ERR invalid-state-or-command\n") == 0);
    execute(&same, 3, 3);
    execute(&other_domain, 2, 4);
    execute(&changed, 0, 5);
    struct qemu_plugin_tb new_block = block(0x5000, 0x6000, 0x90);
    execute(&new_block, 1, 6);
    assert(strcmp(command("end\n"), "OK end\n") == 0);
    assert(profile.blocks[0].count == 3);
    assert(profile.blocks[1].count == 4);
    assert(profile.blocks[2].count == 5);
    assert(profile.blocks[3].count == 6);
    output_contains("complete=1 invalid=0");
    output_contains("total-tb-entries=18 total-translated-slot-weight=36 tracked-tb-entries=18 tracked-translated-slot-weight=36");
    output_contains("# end rows=4 populated=4");
    assert(strcmp(command("end\n"), "ERR invalid-state-or-command\n") == 0);
    teardown();
}
static void budget_and_incomplete(void) {
    setup(1, 1);
    struct qemu_plugin_tb a = block(0x1000, 0x2000, 0x90);
    struct qemu_plugin_tb missing = block(0x2000, 0x3000, 0x91);
    assert(profile.used == 1 && profile.dropped == 1 && missing.operations == 2);
    assert(begin_phase());
    execute(&a, 0, 3);
    execute(&missing, 1, 10);
    assert(!export_phase(false));
    output_contains("complete=0 invalid=0 capacity=1 used=1 dropped-translations=1");
    output_contains("total-tb-entries=13 total-translated-slot-weight=26 tracked-tb-entries=3 tracked-translated-slot-weight=6");
    teardown();
}
static void overflow_and_regression(void) {
    setup(1, 1);
    set_counter(0, 0, UINT64_MAX);
    set_counter(0, 1, 1);
    assert(!begin_phase());
    assert(profile.invalid);
    teardown();
    setup(1, 1);
    struct qemu_plugin_tb a = block(0x1000, 0x2000, 0x90);
    execute(&a, 0, 10);
    assert(begin_phase());
    set_counter(0, 0, 0);
    assert(!export_phase(true));
    output_contains("complete=1 invalid=1");
    teardown();
}
static void parsing_and_order(void) {
    size_t value = 9;
    for (size_t i = 0; i < 7; ++i) {
        const char *bad[] = {"", "-1", "+1", "0", " 1", "9x", "999999999999999999999999"};
        assert(!number(bad[i], 128, &value));
    }
    assert(!number("129", 128, &value));
    assert(number("128", 128, &value) && value == 128);
    setup(2, 1);
    struct qemu_plugin_tb b = block(0x2000, 0x3000, 0x90);
    struct qemu_plugin_tb a = block(0x1000, 0x2000, 0x90);
    assert(begin_phase());
    execute(&a, 0, 4);
    execute(&b, 1, 4);
    assert(export_phase(true));
    output_contains("0000000000001000\t0000000000002000");
    output_contains("# end rows=1 populated=2");
    teardown();
}
static void socket_lifecycle(void) {
    memset(&profile, 0, sizeof(profile));
    char directory[] = "/private/tmp/nanami-tcg-test.XXXXXX";
    assert(mkdtemp(directory) != NULL);
    char output[256], path[256], output_arg[272], socket_arg[272];
    assert(snprintf(output, sizeof(output), "%s/counts.tsv", directory) > 0);
    assert(snprintf(path, sizeof(path), "%s/control.sock", directory) > 0);
    snprintf(output_arg, sizeof(output_arg), "output=%s", output);
    snprintf(socket_arg, sizeof(socket_arg), "socket=%s", path);
    char *args[] = {output_arg, socket_arg, "max=4", "top=2"};
    qemu_info_t info = {.target_name = "x86_64", .system_emulation = true};
    info.system.max_vcpus = 4;
    int installed = qemu_plugin_install(1, &info, 4, args);
    if (installed != 0) {
        perror("plugin install");
    }
    assert(installed == 0);
    struct stat status;
    assert(stat(path, &status) == 0 && (status.st_mode & 0777) == 0600);
    for (size_t i = 0; i < 2; ++i) {
        int fd = socket(AF_UNIX, SOCK_STREAM, 0);
        assert(fd >= 0);
        struct sockaddr_un address = {0};
        address.sun_family = AF_UNIX;
        assert(strlen(path) < sizeof(address.sun_path));
        memcpy(address.sun_path, path, strlen(path) + 1);
        assert(connect(fd, (struct sockaddr *)&address, sizeof(address)) == 0);
        const char *request = i == 0 ? "begin\n" : "end\n";
        assert(write(fd, request, 2) == 2);
        assert(write(fd, request + 2, strlen(request) - 2) == (ssize_t)strlen(request) - 2);
        char reply[64] = {0};
        size_t used = 0;
        while (memchr(reply, '\n', used) == NULL) {
            assert(used + 1 < sizeof(reply));
            ssize_t bytes = read(fd, reply + used, sizeof(reply) - used - 1);
            assert(bytes > 0);
            used += (size_t)bytes;
        }
        assert(strcmp(reply, i == 0 ? "OK begin\n" : "OK end\n") == 0);
        close(fd);
    }
    finish(NULL);
    assert(lstat(path, &status) != 0 && errno == ENOENT);
    assert(unlink(output) == 0);
    assert(rmdir(directory) == 0);
}
int main(void) {
    identity_and_phase();
    budget_and_incomplete();
    overflow_and_regression();
    parsing_and_order();
    socket_lifecycle();
    puts("TCG-PROFILE-TEST-PASS grouping=true phases=true bounds=true overflow=true socket=true");
    return 0;
}
