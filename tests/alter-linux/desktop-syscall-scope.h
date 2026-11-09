/* Diagnostic owner is the already-running X observer, never the execing app.
 * Start precedes observer READY; stop follows the first verified ping reply.
 * This covers all Linux dispatch in this Alter, including background clients.
 * Serialization/sync follow the ordinary ping report, outside the scope. */
extern int prctl(int option, ...);
extern int fsync(int fd);
extern int unlink(const char *path);
extern int *__errno_location(void);
static int app_scope_mode = -1;
static unsigned long app_scope_packet[1034];

static int app_scope_start(const char *mode) {
    int previous = open("/tmp/app-syscall-scope.bin", 0);
    if (previous >= 0) {
        if (close(previous) != 0 || unlink("/tmp/app-syscall-scope.bin") != 0) {
            return 0;
        }
    } else if (*__errno_location() != 2) {
        return 0;
    }
    if (equal(mode, "off")) {
        app_scope_mode = -1;
        return 1;
    }
    if (!equal(mode, "count") && !equal(mode, "time")) {
        return 0;
    }
    app_scope_mode = equal(mode, "count") ? 0 : 1;
    return prctl(15, app_scope_mode == 0 ? "nscope.count" : "nscope.time", 0UL, 0UL, 0UL) == 0;
}
static int app_scope_stop(void) {
    return app_scope_mode < 0 || prctl(15, "nscope.stop", 0UL, 0UL, 0UL) == 0;
}
static int app_scope_save(int output, const char *label) {
    if (app_scope_mode >= 0) {
        if (prctl(0x4e530001, app_scope_packet, sizeof(app_scope_packet), 0UL, 0UL)
                != (int)sizeof(app_scope_packet)
            || app_scope_packet[0] != 0x00315043534e414eUL
            || app_scope_packet[1] != 1 || app_scope_packet[2] != (unsigned long)app_scope_mode
            || app_scope_packet[4] < app_scope_packet[3]
            || app_scope_packet[5] != 1 || app_scope_packet[6] != 513 || app_scope_packet[7] != 1) {
            return 0;
        }
        int fd = open("/tmp/app-syscall-scope.bin", 1 | 64 | 512, 0600);
        if (fd < 0) {
            return 0;
        }
        unsigned long offset = 0;
        while (offset < sizeof(app_scope_packet)) {
            long count = write(fd, (const char *)app_scope_packet + offset, sizeof(app_scope_packet) - offset);
            if (count <= 0 || (unsigned long)count > sizeof(app_scope_packet) - offset) {
                close(fd);
                return 0;
            }
            offset += (unsigned long)count;
        }
        int synced = fsync(fd);
        int closed = close(fd);
        if (synced != 0 || closed != 0) {
            return 0;
        }
    }
    // Persist stale-file removal too, including the disabled control.
    return fsync(output) == 0 && report(output, "APP-SCOPE-SAVED ", label);
}
