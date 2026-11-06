/* Exercise the actual observer loop and bounded failure-only evidence. */
#include <assert.h>
#include <string.h>
#define APP_OBSERVER_TRACE_FAILURE 1
#define main observer_main
#define open test_open
#define write test_write
#define close test_close
#define poll test_poll
#define clock_gettime test_clock
#define __errno_location test_errno_location
#include "../alter-linux/desktop-window-observer.c"
#undef main

enum { CLOCK_ERROR, DEADLINE, FOUND, BEGIN_ERROR, POLL_ERROR };
static int scenario, clock_calls, consumed, error_number, subscriptions;
static unsigned int tree_queries, short_write, writes;
static int wide_tree;
static char captured[16384];
static unsigned long captured_size;
static WindowId tree_children[64];

static void reset(int next) {
    scenario = next;
    clock_calls = consumed = error_number = subscriptions = 0;
    tree_queries = short_write = writes = 0;
    captured_size = 0;
    captured[0] = 0;
    candidate = observer_root = 0;
    wide_tree = 0;
    memset(&observer_trace, 0, sizeof(observer_trace));
    for (unsigned int i = 0; i < 64; ++i) {
        tree_children[i] = 42 + i;
    }
}
AppDisplay *XOpenDisplay(const char *name) {
    assert(strcmp(name, "unix/:0") == 0);
    return (AppDisplay *)1;
}
int XCloseDisplay(AppDisplay *display) { (void)display; return 0; }
WindowId XDefaultRootWindow(AppDisplay *display) { (void)display; return 1; }
int XConnectionNumber(AppDisplay *display) { (void)display; return 3; }
int XSelectInput(AppDisplay *display, WindowId window, long mask) {
    (void)display; (void)window; (void)mask;
    ++subscriptions;
    return 1;
}
int XQueryTree(AppDisplay *display, WindowId window, WindowId *root,
               WindowId *parent, WindowId **children, unsigned int *count) {
    (void)display;
    ++tree_queries;
    error_number = 123; /* Diagnostics must preserve the original errno. */
    *root = *parent = 1;
    *children = 0;
    *count = 0;
    if (clock_calls != 0 && (window == 1 || wide_tree)) {
        *children = tree_children;
        *count = 1;
        if (wide_tree) {
            *count = 64;
        }
    }
    return 1;
}
int XFetchName(AppDisplay *display, WindowId window, char **title) {
    (void)display;
    *title = "Root";
    if (window != 1) {
        *title = "Xfe (root) - /root";
        if (wide_tree) {
            *title = "[unmatched\nwindow]";
        }
    }
    return 1;
}
int XGetWindowAttributes(AppDisplay *display, WindowId window,
                         struct app_window_attributes *attributes) {
    (void)display; (void)window;
    *attributes = (struct app_window_attributes){0};
    attributes->map_state = 2;
    attributes->width = 740;
    attributes->height = 480;
    return 1;
}
int XFree(void *address) { (void)address; return 1; }
int XPending(AppDisplay *display) {
    (void)display;
    return scenario == FOUND && !consumed;
}
int XNextEvent(AppDisplay *display, void *output) {
    (void)display;
    union app_event *event = output;
    *event = (union app_event){0};
    event->structure.type = 16;
    event->structure.window = 42;
    consumed = 1;
    return 0;
}
int XSync(AppDisplay *display, int discard) { (void)display; (void)discard; return 1; }
int XFlush(AppDisplay *display) { (void)display; return 1; }
unsigned long XInternAtom(AppDisplay *display, const char *name, int only_existing) {
    (void)display; (void)name; (void)only_existing; return 0;
}
int XGetWMProtocols(AppDisplay *display, WindowId window, unsigned long **protocols, int *count) {
    (void)display; (void)window; (void)protocols; (void)count; return 0;
}
int XSendEvent(AppDisplay *display, WindowId window, int propagate, long mask, void *event) {
    (void)display; (void)window; (void)propagate; (void)mask; (void)event; return 0;
}
int (*XSetErrorHandler(int (*handler)(AppDisplay *, void *)))(AppDisplay *, void *) {
    return handler;
}
int test_open(const char *path, int flags, ...) {
    (void)flags;
    assert(strcmp(path, "/tmp/pointer-probe.log") == 0);
    return 7;
}
long test_write(int fd, const void *bytes, unsigned long count) {
    assert(fd == 7);
    ++writes;
    if (short_write != 0 && count > short_write) {
        count = short_write;
    }
    assert(captured_size + count < sizeof(captured));
    memcpy(captured + captured_size, bytes, count);
    captured_size += count;
    captured[captured_size] = 0;
    return (long)count;
}
int test_close(int fd) { assert(fd == 7); return 0; }
int test_poll(struct app_pollfd *fds, unsigned long count, int timeout) {
    assert(count == 1 && fds[0].fd == 3 && timeout == 1000);
    if (scenario == POLL_ERROR) {
        error_number = 4;
        return -1;
    }
    return 0;
}
int test_clock(int clock, struct app_timespec *time) {
    assert(clock == 1);
    ++clock_calls;
    if (scenario == BEGIN_ERROR || (scenario == CLOCK_ERROR && clock_calls > 1)) {
        error_number = 14;
        return -1;
    }
    time->seconds = 100;
    time->nanoseconds = 17;
    if (clock_calls > 1) {
        time->seconds = 190;
    }
    return 0;
}
int *test_errno_location(void) { return &error_number; }

int main(void) {
    reset(DEADLINE);
    union app_event event = {0};
    event.property.type = 28;
    event.property.window = 1234;
    event.property.atom = 39;
    trace_xevent(&event);
    assert(observer_trace.recent[0].type == 28 && observer_trace.recent[0].window == 1234);
    event.client.type = 33;
    event.client.window = 5678;
    trace_xevent(&event);
    assert(observer_trace.recent[1].window == 5678);
    char *argv[] = {"aw", "files", 0};
    for (int mode = CLOCK_ERROR; mode <= POLL_ERROR; ++mode) {
        reset(mode);
        int result = observer_main(2, argv);
        if (mode == FOUND) {
            assert(result == 0);
            assert(strstr(captured, "APP-WINDOW-READY files\n") != 0);
            assert(strstr(captured, "APP-OBSERVER-FAIL") == 0);
            continue;
        }
        assert(strstr(captured, "APP-WINDOW-READY") == 0);
        assert(strstr(captured, "APP-EVENTLOOP-REPLY") == 0);
        if (mode == BEGIN_ERROR) {
            assert(result == 6 && strstr(captured, "stage=begin-clock errno=14") != 0);
        } else if (mode == POLL_ERROR) {
            assert(result == 7 && strstr(captured, "stage=discovery-poll errno=4") != 0);
        } else {
            assert(result == 9);
            if (mode == CLOCK_ERROR) {
                assert(strstr(captured, "stage=discovery-clock errno=14") != 0);
                assert(strstr(captured, "begin-s=100 now-s=0") != 0);
            } else {
                assert(strstr(captured, "stage=discovery-deadline errno=0") != 0);
                assert(strstr(captured, "begin-s=100 now-s=190") != 0);
            }
            assert(strstr(captured, "map=2 width=740 height=480 match=1 title=Xfe") != 0);
            assert(candidate == 0); /* A visible snapshot never changes readiness. */
        }
    }
    reset(DEADLINE);
    clock_calls = 1;
    observer_root = 1;
    candidate = 77;
    wide_tree = 1;
    short_write = 7;
    for (unsigned long i = 0; i < 100; ++i) {
        trace_event(16, 1000 + i);
    }
    trace_failure((AppDisplay *)1, 7, "files", "test-bound", 14, 100, 190);
    assert(captured_size <= 4096 && captured[captured_size - 1] == '\n');
    assert(strchr(captured, '\n') == captured + captured_size - 1);
    assert(strstr(captured, "events=100") != 0 && strstr(captured, "16:1084,") != 0);
    assert(strstr(captured, "16:1083,") == 0);
    assert(strstr(captured, "nodes=24") != 0 && tree_queries <= 24);
    assert(candidate == 77 && subscriptions == 0 && consumed == 0);
    assert(writes > 1);
    return 0;
}
