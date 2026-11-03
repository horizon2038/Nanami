/* Exercise the production retry policy without an X server or sleeping. */
#include <assert.h>
#include <string.h>
#define main readiness_main
#define XOpenDisplay fake_open
#define XCloseDisplay fake_close
#define kill fake_kill
#define nanosleep fake_sleep
#define __errno_location fake_errno
#include "wait-for-x.c"
#undef main

static int opened, closed, checked, slept, failures, dead, interrupted, sleep_error, error;
static int last_pid;
static const char *last_display;
static _Alignas(16) unsigned char display_handle;

Display *fake_open(const char *name) {
    last_display = name;
    ++opened;
    if (opened <= failures) {
        return 0;
    }
    return (Display *)&display_handle;
}
int fake_close(Display *display) {
    assert(display == (Display *)&display_handle);
    ++closed;
    return 0;
}
int fake_kill(int pid, int signal) {
    assert(signal == 0 && pid > 0);
    last_pid = pid;
    ++checked;
    return dead;
}
int fake_sleep(const struct wait_delay *delay, struct wait_delay *remaining) {
    assert(delay->seconds == 0 && delay->nanoseconds > 0 && delay->nanoseconds <= 100000000);
    ++slept;
    if (interrupted) {
        interrupted = 0;
        remaining->seconds = 0;
        remaining->nanoseconds = 50000000;
        error = 4;
        return -1;
    }
    if (sleep_error) {
        error = 22;
        return -1;
    }
    return 0;
}
int *fake_errno(void) {
    return &error;
}
static void reset(void) {
    opened = closed = checked = slept = failures = dead = interrupted = sleep_error = error = 0;
    last_pid = 0;
    last_display = 0;
}

int main(void) {
    char *arguments[] = {"wait-for-x", "unix/:7", "123", 0};
    reset();
    assert(readiness_main(2, arguments) == 2 && opened == 0);
    const char *invalid[] = {"", "0", "-1", "1a", "2147483648", "99999999999999999999"};
    for (unsigned i = 0; i < sizeof(invalid) / sizeof(invalid[0]); ++i) {
        reset();
        arguments[2] = (char *)invalid[i];
        assert(readiness_main(3, arguments) == 2 && opened == 0);
    }
    arguments[2] = "123";
    reset();
    assert(readiness_main(3, arguments) == 0);
    assert(opened == 1 && closed == 1 && checked == 0 && slept == 0);
    assert(strcmp(last_display, "unix/:7") == 0);
    reset();
    failures = 2;
    assert(readiness_main(3, arguments) == 0);
    assert(opened == 3 && closed == 1 && checked == 2 && slept == 2 && last_pid == 123);
    reset();
    failures = 100;
    assert(readiness_main(3, arguments) == 1);
    assert(opened == 30 && closed == 0 && checked == 29 && slept == 29);
    reset();
    failures = 1;
    dead = -1;
    assert(readiness_main(3, arguments) == 1);
    assert(opened == 1 && closed == 0 && checked == 1 && slept == 0);
    reset();
    failures = 1;
    interrupted = 1;
    assert(readiness_main(3, arguments) == 0);
    assert(opened == 2 && closed == 1 && checked == 2 && slept == 2);
    reset();
    failures = 1;
    sleep_error = 1;
    assert(readiness_main(3, arguments) == 1 && opened == 1 && closed == 0);
    reset();
    arguments[2] = "2147483647";
    failures = 1;
    assert(readiness_main(3, arguments) == 0 && last_pid == 2147483647);
    return 0;
}
