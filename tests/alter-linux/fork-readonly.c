/* RO fork aliases, write upgrades and parked CLONE_VM waiters: integer-only. */
typedef unsigned long word;
extern long linux_call(long, word, word, word, word, word, word);
extern long seal_clone_vm(void *stack);
static volatile word *pages;
static int channel[2];
static volatile word waiter_done;
static volatile word spin_mode;
static volatile word spin_done;
static const word length = 2 * 1024 * 1024;
static long call(long n, word a, word b, word c, word d) {
    return linux_call(n, a, b, c, d, 0, 0);
}
static int wait_child(long pid) {
    int status = -1;
    return pid > 0 && call(61, (word)pid, (word)&status, 0, 0) == pid && status == 0;
}
static void child_exit(int status) {
    call(60, (word)status, 0, 0, 0);
}
int seal_shared_child(void) {
    if (spin_mode != 0) {
        while (spin_done == 0) {
            /* Deliberately no syscall: the supervisor must stop a running peer. */
        }
        return 0;
    }
    char byte = 0;
    if (call(0, (word)channel[0], (word)&byte, 1, 0) != 1 || byte != 'x') {
        return 80;
    }
    waiter_done += 1;
    return 0;
}
int test_main(void) {
    long address = linux_call(9, 0, length, 3, 0x22, (word)-1, 0);
    if (address < 0) {
        return 10;
    }
    pages = (volatile word *)address;
    for (word offset = 0; offset < length / sizeof(word); offset += 512) {
        pages[offset] = 0x13570000 + offset;
    }
    if (call(10, (word)pages, length, 1, 0) != 0) {
        return 11;
    }
    for (word round = 0; round < 3; ++round) {
        long child = call(57, 0, 0, 0, 0);
        if (child == 0) {
            if (call(10, (word)pages, length, 0, 0) != 0 || call(10, (word)pages, length, 3, 0) != 0) {
                child_exit(20);
            }
            for (word offset = 0; offset < length / sizeof(word); offset += 512) {
                if (pages[offset] != 0x13570000 + offset) {
                    child_exit(21);
                }
                pages[offset] += 1;
            }
            if (call(10, (word)pages, length, 1, 0) != 0) {
                child_exit(22);
            }
            long descendant = call(57, 0, 0, 0, 0);
            if (descendant == 0) {
                if (call(10, (word)pages, 4096, 3, 0) != 0 || pages[0] != 0x13570001) {
                    child_exit(23);
                }
                pages[0] = 99;
                child_exit(0);
            }
            if (!wait_child(descendant) || pages[0] != 0x13570001) {
                child_exit(24);
            }
            child_exit(0);
        }
        if (!wait_child(child)) {
            return 12;
        }
        for (word offset = 0; offset < length / sizeof(word); offset += 512) {
            if (pages[offset] != 0x13570000 + offset) {
                return 13;
            }
        }
    }
    /* Leave a shared-VM peer parked inside read while the parent detaches.
       It must remain blocked; Suspend/Resume would replay a handled syscall. */
    if (call(22, (word)channel, 0, 0, 0) != 0) {
        return 30;
    }
    long stack = linux_call(9, 0, 65536, 3, 0x22, (word)-1, 0);
    if (stack < 0) {
        return 31;
    }
    long waiter = seal_clone_vm((void *)((word)stack + 65536));
    word delay[2] = { 0, 20000000 };
    call(35, (word)delay, 0, 0, 0);
    long child = call(57, 0, 0, 0, 0);
    if (child == 0) {
        call(35, (word)delay, 0, 0, 0);
        if (pages[0] != 0x13570000) {
            child_exit(32);
        }
        child_exit(0);
    }
    if (child < 0 || call(10, (word)pages, 4096, 3, 0) != 0 || waiter_done != 0) {
        return 33;
    }
    pages[0] = 88;
    if (!wait_child(child) || waiter_done != 0) {
        return 34;
    }
    const char byte = 'x';
    if (call(1, (word)channel[1], (word)&byte, 1, 0) != 1 || !wait_child(waiter) || waiter_done != 1) {
        return 35;
    }
    spin_mode = 1;
    if (call(10, (word)pages, 4096, 1, 0) != 0) {
        return 40;
    }
    waiter = seal_clone_vm((void *)((word)stack + 65536));
    child = call(57, 0, 0, 0, 0);
    if (child == 0) {
        call(35, (word)delay, 0, 0, 0);
        if (pages[0] != 88) {
            child_exit(41);
        }
        child_exit(0);
    }
    if (child < 0 || call(10, (word)pages, 4096, 3, 0) != 0) {
        return 42;
    }
    pages[0] = 89;
    spin_done = 1;
    if (!wait_child(waiter) || !wait_child(child)) {
        return 43;
    }
    if (call(11, (word)pages, length, 0, 0) != 0 || call(11, (word)stack, 65536, 0, 0) != 0) {
        return 36;
    }
    call(3, (word)channel[0], 0, 0, 0);
    call(3, (word)channel[1], 0, 0, 0);
    static const char pass[] = "FORK-READONLY-PASS isolation=true descendant=true parked-vm=true running-vm=true\n";
    call(1, 1, (word)pass, sizeof(pass) - 1, 0);
    return 0;
}
