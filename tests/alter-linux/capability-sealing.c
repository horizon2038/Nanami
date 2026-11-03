/* Native capability access must stay absent through fork, CLONE_VM and exec. */
typedef unsigned long word;
extern long linux_call(long, word, word, word, word, word, word);
extern long seal_clone_vm(void *stack);
extern int seal_probe_capability(word descriptor);
static volatile word *shared;

static int empty_capability_root(void) {
    for (word slot = 0; slot < 256; ++slot) {
        const word descriptor = 0x0800000000000000UL | (slot << 48);
        if (seal_probe_capability(descriptor) != 0) {
            return 1;
        }
    }
    /* Also address both slots using the new one-bit root's exact depth. */
    for (word slot = 0; slot < 2; ++slot) {
        if (seal_probe_capability(0x0900000000000000UL | (slot << 55)) != 0) {
            return 1;
        }
    }
    return 0;
}

int seal_shared_child(void) {
    if (empty_capability_root() != 0) {
        return 21;
    }
    *shared = 99;
    return 0;
}

static int wait_child(long child) {
    int status = -1;
    if (child <= 0 || linux_call(61, (word)child, (word)&status, 0, 0, 0, 0) != child) {
        return 1;
    }
    return status != 0;
}

int test_main(void) {
    if (empty_capability_root() != 0) {
        return 10;
    }
    /* The deliberately inherited FD distinguishes the re-executed child. */
    if (linux_call(72, 12, 1, 0, 0, 0, 0) >= 0) {
        return 0;
    }
    long page = linux_call(9, 0, 4096, 3, 0x22, (word)-1, 0);
    if (page < 0) {
        return 11;
    }
    shared = (volatile word *)page;
    *shared = 42;
    if (linux_call(10, (word)page, 4096, 1, 0, 0, 0) != 0
        || linux_call(10, (word)page, 4096, 3, 0, 0, 0) != 0) {
        return 12;
    }
    int pipe[2];
    if (linux_call(22, (word)pipe, 0, 0, 0, 0, 0) != 0) {
        return 13;
    }
    const word readonly_size = 2 * 1024 * 1024;
    long readonly_address = linux_call(9, 0, readonly_size, 3, 0x22, (word)-1, 0);
    if (readonly_address < 0) {
        return 22;
    }
    volatile word *readonly = (volatile word *)readonly_address;
    for (word page = 0; page < readonly_size / sizeof(word); page += 512) {
        readonly[page] = 0x12340000 + page;
    }
    if (linux_call(10, (word)readonly, readonly_size, 1, 0, 0, 0) != 0) {
        return 23;
    }
    long child = linux_call(57, 0, 0, 0, 0, 0, 0);
    if (child == 0) {
        if (empty_capability_root() != 0 || *shared != 42) {
            linux_call(60, 14, 0, 0, 0, 0, 0);
        }
        *shared = 17;
        if (linux_call(33, (word)pipe[0], 12, 0, 0, 0, 0) != 12) {
            linux_call(60, 15, 0, 0, 0, 0, 0);
        }
        const char *arguments[] = { "/bin/nanami-cap-seal", 0 };
        const char *environment[] = { 0 };
        const char missing[] = "/bin/nanami-no-such-executable";
        if (linux_call(59, (word)missing, (word)arguments, (word)environment, 0, 0, 0) != -2
            || readonly[0] != 0x12340000) {
            linux_call(60, 24, 0, 0, 0, 0, 0);
        }
        linux_call(59, (word)arguments[0], (word)arguments, (word)environment, 0, 0, 0);
        linux_call(60, 16, 0, 0, 0, 0, 0);
        return 16;
    }
    if (wait_child(child) != 0 || *shared != 42) {
        return 17;
    }
    /* Child exec must retire only its own Frame aliases, not our RO backing. */
    for (word page = 0; page < readonly_size / sizeof(word); page += 512) {
        if (readonly[page] != 0x12340000 + page) {
            return 25;
        }
    }
    if (linux_call(11, (word)readonly, readonly_size, 0, 0, 0, 0) != 0) {
        return 26;
    }
    long stack = linux_call(9, 0, 65536, 3, 0x22, (word)-1, 0);
    if (stack < 0 || wait_child(seal_clone_vm((void *)((word)stack + 65536))) != 0
        || *shared != 99) {
        return 18;
    }
    if (linux_call(11, (word)stack, 65536, 0, 0, 0, 0) != 0
        || linux_call(11, (word)page, 4096, 0, 0, 0, 0) != 0) {
        return 19;
    }
    const char pass[] = "CAPABILITY-SEAL-PASS fork=true shared-vm=true exec=true\n";
    linux_call(1, 1, (word)pass, sizeof(pass) - 1, 0, 0, 0);
    return 0;
}
