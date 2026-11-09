/* Exercise actual test-only owner protocol without X11 or privileged calls. */
#include <assert.h>
#include <stdarg.h>
#include <string.h>
static int equal(const char *a, const char *b) { return strcmp(a, b) == 0; }
static int saved, opened, closed, synced, controls, failure, error;
static unsigned long written;
static int report(int fd, const char *prefix, const char *label) {
    assert(fd == 9 && equal(prefix, "APP-SCOPE-SAVED ") && equal(label, "files"));
    ++saved;
    return 1;
}
extern int open(const char *path, int flags, ...);
extern long write(int fd, const void *bytes, unsigned long count);
extern int close(int fd);
#include "../alter-linux/desktop-syscall-scope.h"
int *__errno_location(void) { return &error; }
int unlink(const char *path) {
    assert(equal(path, "/tmp/app-syscall-scope.bin"));
    error = failure == 7 ? 13 : 2;
    return failure == 9 ? 0 : -1;
}
int prctl(int option, ...) {
    va_list args;
    va_start(args, option);
    ++controls;
    if (option == 15) {
        const char *name = va_arg(args, const char *);
        assert(equal(name, "nscope.count") || equal(name, "nscope.time") || equal(name, "nscope.stop"));
        va_end(args);
        return failure == 1 ? -1 : 0;
    }
    assert(option == 0x4e530001);
    unsigned long *p = va_arg(args, unsigned long *);
    unsigned long size = va_arg(args, unsigned long);
    assert(size == sizeof(app_scope_packet));
    memset(p, 0, size);
    p[0] = 0x00315043534e414eUL; p[1] = 1; p[2] = app_scope_mode;
    p[3] = 10; p[4] = 20; p[5] = 1; p[6] = 513; p[7] = 1;
    if (failure == 2) { p[5] = 0; }
    va_end(args);
    return failure == 3 ? -1 : (int)size;
}
int open(const char *path, int flags, ...) {
    assert(equal(path, "/tmp/app-syscall-scope.bin"));
    if (flags == 0) {
        error = failure == 8 ? 13 : 2;
        return failure == 7 || failure == 9 ? 10 : -1;
    }
    assert(flags == (1 | 64 | 512));
    ++opened;
    return 10;
}
long write(int fd, const void *bytes, unsigned long count) {
    assert(fd == 10 && bytes == (const char *)app_scope_packet + written);
    assert(count == sizeof(app_scope_packet) - written);
    if (failure == 4) { return 0; }
    if (count > 37) { count = 37; }
    written += count;
    return (long)count;
}
int fsync(int fd) { assert(fd == 10 || fd == 9); ++synced; return failure == 5 ? -1 : 0; }
int close(int fd) { assert(fd == 10); ++closed; return failure == 6 ? -1 : 0; }
static void reset(int fail) {
    saved = opened = closed = synced = controls = 0;
    written = 0; failure = fail;
}
int main(void) {
    for (int mode = 0; mode < 2; ++mode) {
        reset(0);
        assert(app_scope_start(mode == 0 ? "count" : "time"));
        assert(app_scope_stop());
        assert(app_scope_save(9, "files"));
        assert(saved == 1 && opened == 1 && closed == 1 && synced == 2);
        assert(written == sizeof(app_scope_packet) && controls == 3);
    }
    reset(0);
    assert(!app_scope_start("invalid"));
    assert(app_scope_start("off") && app_scope_stop() && app_scope_save(9, "files"));
    assert(saved == 1 && controls == 0 && opened == 0);
    reset(9);
    assert(app_scope_start("count") && app_scope_stop() && app_scope_save(9, "files"));
    assert(closed == 2 && saved == 1); /* stale read handle and report handle */
    for (int fail = 1; fail <= 8; ++fail) {
        reset(fail);
        int ok = app_scope_start("time") && app_scope_stop() && app_scope_save(9, "files");
        assert(!ok && saved == 0);
        if (fail >= 4 && fail <= 6) { assert(closed == 1); }
    }
    return 0;
}
