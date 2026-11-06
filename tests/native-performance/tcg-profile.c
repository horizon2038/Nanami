/* Diagnostic host plugin, not a guest or Kernel component.
 * Snapshots require the controller to stop every vCPU and await its ACK.
 * Counter weights count translated slots at TB entry, not retired instructions
 * or CPU time: an exception can interrupt a block, and REP can reenter it. */
#define _POSIX_C_SOURCE 200809L
#include <qemu-plugin.h>
#include <errno.h>
#include <poll.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdatomic.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/un.h>
#include <unistd.h>

enum { DEFAULT_ENTRIES = 65536, MAX_ENTRIES = 131072, MAX_ROWS = 512 };
struct block_key {
    uint64_t pc, physical, code_hash, instructions;
};
struct block {
    struct block_key key;
    uint64_t baseline, count, weight;
    size_t slot, sample_bytes;
    unsigned char sample[16];
};
enum phase { IDLE, ACTIVE, FINISHED };
struct profile {
    GMutex mutex;
    GHashTable *index;
    struct block *blocks;
    struct qemu_plugin_scoreboard *scores;
    size_t capacity, used, rows;
    uint64_t base_entries, base_weight, dropped, base_dropped;
    unsigned int max_cpus;
    enum phase phase;
    bool invalid, io_error, socket_owned;
    FILE *output;
    int listener;
    char *socket_path;
    GThread *worker;
    atomic_bool stopping;
};
static struct profile profile;
int qemu_plugin_version = QEMU_PLUGIN_VERSION;

static guint key_hash(gconstpointer data) {
    const struct block_key *key = data;
    uint64_t value = key->pc ^ key->physical ^ key->code_hash ^ key->instructions;
    return (guint)(value ^ (value >> 32));
}
static gboolean key_equal(gconstpointer left, gconstpointer right) {
    const struct block_key *a = left, *b = right;
    return a->pc == b->pc && a->physical == b->physical
        && a->code_hash == b->code_hash && a->instructions == b->instructions;
}
static qemu_plugin_u64 counter(size_t slot) {
    return (qemu_plugin_u64){profile.scores, slot * sizeof(uint64_t)};
}
static bool add_checked(uint64_t *value, uint64_t added) {
    if (UINT64_MAX - *value < added) {
        profile.invalid = true;
        return false;
    }
    *value += added;
    return true;
}
static uint64_t sum_counter(size_t slot) {
    uint64_t result = 0;
    int cpus = qemu_plugin_num_vcpus();
    if (cpus < 0 || (unsigned int)cpus > profile.max_cpus) {
        profile.invalid = true;
        return 0;
    }
    for (unsigned int cpu = 0; cpu < (unsigned int)cpus; ++cpu) {
        if (!add_checked(&result, qemu_plugin_u64_get(counter(slot), cpu))) {
            return 0;
        }
    }
    return result;
}
static uint64_t delta(uint64_t current, uint64_t baseline) {
    if (current < baseline) {
        profile.invalid = true;
        return 0;
    }
    return current - baseline;
}
static void translate(struct qemu_plugin_tb *tb, void *unused) {
    (void)unused;
    size_t count = qemu_plugin_tb_n_insns(tb);
    if (count == 0) {
        return;
    }
    struct qemu_plugin_insn *first = qemu_plugin_tb_get_insn(tb, 0);
    struct block_key key = {
        qemu_plugin_tb_vaddr(tb), (uintptr_t)qemu_plugin_insn_haddr(first),
        UINT64_C(14695981039346656037), count,
    };
    for (size_t i = 0; i < count; ++i) {
        unsigned char data[16];
        struct qemu_plugin_insn *insn = qemu_plugin_tb_get_insn(tb, i);
        size_t bytes = qemu_plugin_insn_data(insn, data, sizeof(data));
        for (size_t j = 0; j < bytes; ++j) {
            key.code_hash = (key.code_hash ^ data[j]) * UINT64_C(1099511628211);
        }
    }
    /* Total counts include blocks rejected by the metadata budget. */
    qemu_plugin_register_vcpu_tb_exec_inline_per_vcpu(tb,
        QEMU_PLUGIN_INLINE_ADD_U64, counter(0), 1);
    qemu_plugin_register_vcpu_tb_exec_inline_per_vcpu(tb,
        QEMU_PLUGIN_INLINE_ADD_U64, counter(1), count);
    g_mutex_lock(&profile.mutex);
    struct block *block = g_hash_table_lookup(profile.index, &key);
    if (block == NULL && profile.used < profile.capacity) {
        block = &profile.blocks[profile.used];
        block->key = key;
        block->slot = profile.used++ + 2;
        block->sample_bytes = qemu_plugin_insn_data(first, block->sample, sizeof(block->sample));
        g_hash_table_insert(profile.index, &block->key, block);
    }
    if (block != NULL) {
        qemu_plugin_register_vcpu_tb_exec_inline_per_vcpu(tb,
            QEMU_PLUGIN_INLINE_ADD_U64, counter(block->slot), 1);
    } else {
        ++profile.dropped;
    }
    g_mutex_unlock(&profile.mutex);
}
static int rank_blocks(const void *left, const void *right) {
    const struct block *a = *(const struct block *const *)left;
    const struct block *b = *(const struct block *const *)right;
    if (a->weight != b->weight) {
        return a->weight < b->weight ? 1 : -1;
    }
    if (a->key.pc != b->key.pc) {
        return a->key.pc < b->key.pc ? -1 : 1;
    }
    if (a->key.physical != b->key.physical) {
        return a->key.physical < b->key.physical ? -1 : 1;
    }
    if (a->key.code_hash != b->key.code_hash) {
        return a->key.code_hash < b->key.code_hash ? -1 : 1;
    }
    if (a->key.instructions != b->key.instructions) {
        return a->key.instructions < b->key.instructions ? -1 : 1;
    }
    return 0;
}
static bool begin_phase(void) {
    if (profile.phase != IDLE) {
        return false;
    }
    profile.base_entries = sum_counter(0);
    profile.base_weight = sum_counter(1);
    profile.base_dropped = profile.dropped;
    for (size_t i = 0; i < profile.used; ++i) {
        profile.blocks[i].baseline = sum_counter(profile.blocks[i].slot);
    }
    profile.phase = ACTIVE;
    return !profile.invalid;
}
static bool export_phase(bool complete) {
    if (profile.phase != ACTIVE) {
        return false;
    }
    uint64_t total_entries = delta(sum_counter(0), profile.base_entries);
    uint64_t total_weight = delta(sum_counter(1), profile.base_weight);
    uint64_t tracked_entries = 0, tracked_weight = 0;
    struct block **ranked = g_new(struct block *, profile.used);
    size_t populated = 0;
    for (size_t i = 0; i < profile.used; ++i) {
        struct block *block = &profile.blocks[i];
        block->count = delta(sum_counter(block->slot), block->baseline);
        if (block->count > UINT64_MAX / block->key.instructions) {
            profile.invalid = true;
            block->weight = 0;
        } else {
            block->weight = block->count * block->key.instructions;
        }
        add_checked(&tracked_entries, block->count);
        add_checked(&tracked_weight, block->weight);
        if (block->count != 0) {
            ranked[populated++] = block;
        }
    }
    if (tracked_entries > total_entries || tracked_weight > total_weight) {
        profile.invalid = true;
    }
    if (populated > 1) {
        qsort(ranked, populated, sizeof(*ranked), rank_blocks);
    }
    fprintf(profile.output, "# tcg-profile-v1 complete=%u invalid=%u capacity=%zu used=%zu dropped-translations=%" PRIu64 " phase-dropped-translations=%" PRIu64 "\n",
        complete, profile.invalid, profile.capacity, profile.used, profile.dropped,
        profile.dropped - profile.base_dropped);
    fprintf(profile.output, "# total-tb-entries=%" PRIu64 " total-translated-slot-weight=%" PRIu64 " tracked-tb-entries=%" PRIu64 " tracked-translated-slot-weight=%" PRIu64 "\n",
        total_entries, total_weight, tracked_entries, tracked_weight);
    fprintf(profile.output, "# stopped-vcpu snapshots; diagnostic counts, not CPU time or retired instructions\n");
    fprintf(profile.output, "pc\tphysical\tcode-hash\ttranslated-insns\ttb-entries\ttranslated-slot-weight\tfirst-insn-bytes\n");
    size_t rows = MIN(populated, profile.rows);
    for (size_t i = 0; i < rows; ++i) {
        const struct block *block = ranked[i];
        fprintf(profile.output, "%016" PRIx64 "\t%016" PRIx64 "\t%016" PRIx64 "\t%" PRIu64 "\t%" PRIu64 "\t%" PRIu64 "\t",
            block->key.pc, block->key.physical, block->key.code_hash,
            block->key.instructions, block->count, block->weight);
        for (size_t j = 0; j < block->sample_bytes; ++j) {
            fprintf(profile.output, "%02x", block->sample[j]);
        }
        fputc('\n', profile.output);
    }
    fprintf(profile.output, "# end rows=%zu populated=%zu\n", rows, populated);
    if (fflush(profile.output) != 0 || ferror(profile.output)) {
        profile.io_error = true;
    }
    g_free(ranked);
    profile.phase = FINISHED;
    return complete && !profile.invalid && !profile.io_error;
}
static const char *command(const char *request) {
    const char *reply = "ERR invalid-state-or-command\n";
    g_mutex_lock(&profile.mutex);
    if (strcmp(request, "begin\n") == 0 && begin_phase()) {
        reply = "OK begin\n";
    } else if (strcmp(request, "end\n") == 0 && export_phase(true)) {
        reply = "OK end\n";
    }
    g_mutex_unlock(&profile.mutex);
    return reply;
}
static void serve_client(int fd) {
#ifdef SO_NOSIGPIPE
    int enabled = 1;
    setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &enabled, sizeof(enabled));
#endif
    char request[32];
    size_t used = 0;
    while (used + 1 < sizeof(request)) {
        struct pollfd wait = {fd, POLLIN, 0};
        if (poll(&wait, 1, 2000) <= 0) {
            return;
        }
        ssize_t count = read(fd, request + used, sizeof(request) - used - 1);
        if (count <= 0) {
            return;
        }
        used += (size_t)count;
        if (memchr(request, '\n', used) != NULL) {
            request[used] = 0;
            const char *reply = command(request);
#ifdef MSG_NOSIGNAL
            send(fd, reply, strlen(reply), MSG_NOSIGNAL);
#else
            send(fd, reply, strlen(reply), 0);
#endif
            return;
        }
    }
}
static gpointer control_worker(gpointer unused) {
    (void)unused;
    while (!atomic_load(&profile.stopping)) {
        struct pollfd wait = {profile.listener, POLLIN, 0};
        if (poll(&wait, 1, 100) > 0) {
            int fd = accept(profile.listener, NULL, NULL);
            if (fd >= 0) {
                serve_client(fd);
                close(fd);
            }
        }
    }
    return NULL;
}
static void finish(void *unused) {
    (void)unused;
    atomic_store(&profile.stopping, true);
    if (profile.worker != NULL) {
        g_thread_join(profile.worker);
    }
    g_mutex_lock(&profile.mutex);
    if (profile.phase == ACTIVE) {
        export_phase(false);
    } else if (profile.phase == IDLE) {
        fputs("# tcg-profile-v1 complete=0 phase=not-started\n", profile.output);
    }
    g_mutex_unlock(&profile.mutex);
    fclose(profile.output);
    if (profile.listener >= 0) {
        close(profile.listener);
    }
    if (profile.socket_owned) {
        unlink(profile.socket_path);
    }
    g_free(profile.socket_path);
    g_hash_table_destroy(profile.index);
    g_free(profile.blocks);
    qemu_plugin_scoreboard_free(profile.scores);
    g_mutex_clear(&profile.mutex);
}
static bool number(const char *text, size_t limit, size_t *value) {
    if (*text == 0 || *text == '-' || *text == '+') {
        return false;
    }
    for (const char *at = text; *at != 0; ++at) {
        if (*at < '0' || *at > '9') {
            return false;
        }
    }
    errno = 0;
    char *end;
    unsigned long parsed = strtoul(text, &end, 10);
    if (errno != 0 || *end != 0 || parsed == 0 || parsed > limit) {
        return false;
    }
    *value = (size_t)parsed;
    return true;
}
int qemu_plugin_install(qemu_plugin_id_t id, const qemu_info_t *info,
                        int argc, char **argv) {
    const char *output = NULL, *socket_path = NULL;
    profile.capacity = DEFAULT_ENTRIES;
    profile.rows = MAX_ROWS;
    profile.listener = -1;
    if (!info->system_emulation || strcmp(info->target_name, "x86_64") != 0
        || info->system.max_vcpus <= 0 || info->system.max_vcpus > 64) {
        return 1;
    }
    profile.max_cpus = (unsigned int)info->system.max_vcpus;
    for (int i = 0; i < argc; ++i) {
        if (strncmp(argv[i], "output=", 7) == 0 && output == NULL) {
            output = argv[i] + 7;
        } else if (strncmp(argv[i], "socket=", 7) == 0 && socket_path == NULL) {
            socket_path = argv[i] + 7;
        } else if (strncmp(argv[i], "max=", 4) == 0) {
            if (!number(argv[i] + 4, MAX_ENTRIES, &profile.capacity)) {
                return 1;
            }
        } else if (strncmp(argv[i], "top=", 4) == 0) {
            if (!number(argv[i] + 4, MAX_ROWS, &profile.rows)) {
                return 1;
            }
        } else {
            return 1;
        }
    }
    struct sockaddr_un address = {0};
    struct stat existing;
    if (output == NULL || *output == 0 || socket_path == NULL || *socket_path == 0
        || strlen(socket_path) >= sizeof(address.sun_path)
        || lstat(socket_path, &existing) == 0 || errno != ENOENT) {
        return 1;
    }
    profile.output = fopen(output, "wx");
    if (profile.output == NULL) {
        return 1;
    }
    profile.listener = socket(AF_UNIX, SOCK_STREAM, 0);
    address.sun_family = AF_UNIX;
    memcpy(address.sun_path, socket_path, strlen(socket_path) + 1);
    if (profile.listener < 0
        || bind(profile.listener, (struct sockaddr *)&address, sizeof(address)) != 0) {
        if (profile.listener >= 0) {
            close(profile.listener);
        }
        fclose(profile.output);
        return 1;
    }
    profile.socket_path = g_strdup(socket_path);
    profile.socket_owned = true;
    if (chmod(socket_path, 0600) != 0 || listen(profile.listener, 4) != 0) {
        close(profile.listener);
        unlink(socket_path);
        g_free(profile.socket_path);
        fclose(profile.output);
        return 1;
    }
    profile.blocks = g_new0(struct block, profile.capacity);
    profile.index = g_hash_table_new(key_hash, key_equal);
    profile.scores = qemu_plugin_scoreboard_new((profile.capacity + 2) * sizeof(uint64_t));
    g_mutex_init(&profile.mutex);
    profile.worker = g_thread_new("tcg-profile-control", control_worker, NULL);
    qemu_plugin_register_vcpu_tb_trans_cb(id, translate, NULL);
    qemu_plugin_register_atexit_cb(id, finish, NULL);
    return 0;
}
