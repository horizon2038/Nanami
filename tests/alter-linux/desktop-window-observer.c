/* Test-only X11 window observer. The launcher and application stay unchanged.
 * LP64 declarations match Tiny Core's existing libc/Xlib, like wait-for-x.c. */
typedef unsigned long WindowId;
typedef struct _XDisplay AppDisplay;
struct app_window_attributes {
    int x, y, width, height, border_width, depth;
    void *visual;
    WindowId root;
    int window_class, bit_gravity, win_gravity, backing_store;
    unsigned long backing_planes, backing_pixel;
    int save_under;
    unsigned long colormap;
    int map_installed, map_state;
    long all_event_masks, your_event_mask, do_not_propagate_mask;
    int override_redirect;
    void *screen;
};

struct app_structure_event {
    int type;
    unsigned long serial;
    int send_event;
    AppDisplay *display;
    WindowId parent, window;
};
struct app_property_event {
    int type;
    unsigned long serial;
    int send_event;
    AppDisplay *display;
    WindowId window;
    unsigned long atom;
};
struct app_client_event {
    int type;
    unsigned long serial;
    int send_event;
    AppDisplay *display;
    WindowId window;
    unsigned long message_type;
    int format;
    long data[5];
};
union app_event {
    long storage[24];
    struct app_structure_event structure;
    struct app_property_event property;
    struct app_client_event client;
};

#ifdef APP_OBSERVER_CHECK_LAYOUT
#include <X11/Xlib.h>
#include <stddef.h>
_Static_assert(sizeof(struct app_window_attributes) == sizeof(XWindowAttributes), "Xlib layout");
_Static_assert(offsetof(struct app_window_attributes, map_state) == offsetof(XWindowAttributes, map_state), "map state");
_Static_assert(sizeof(union app_event) == sizeof(XEvent), "X event storage");
_Static_assert(offsetof(struct app_structure_event, window) == offsetof(XCreateWindowEvent, window), "create window");
_Static_assert(offsetof(struct app_structure_event, window) == offsetof(XMapEvent, window), "map window");
_Static_assert(offsetof(struct app_structure_event, window) == offsetof(XReparentEvent, window), "reparent window");
_Static_assert(offsetof(struct app_property_event, atom) == offsetof(XPropertyEvent, atom), "property atom");
_Static_assert(sizeof(struct app_client_event) == sizeof(XClientMessageEvent), "client event");
_Static_assert(offsetof(struct app_client_event, data) == offsetof(XClientMessageEvent, data), "client data");
#else
extern AppDisplay *XOpenDisplay(const char *name);
extern int XCloseDisplay(AppDisplay *display);
extern WindowId XDefaultRootWindow(AppDisplay *display);
extern int XConnectionNumber(AppDisplay *display);
extern int XSelectInput(AppDisplay *display, WindowId window, long mask);
extern int XQueryTree(AppDisplay *display, WindowId window, WindowId *root,
                      WindowId *parent, WindowId **children, unsigned int *count);
extern int XFetchName(AppDisplay *display, WindowId window, char **name);
extern int XGetWindowAttributes(AppDisplay *display, WindowId window,
                                struct app_window_attributes *attributes);
extern int XFree(void *memory);
extern int XPending(AppDisplay *display);
extern int XNextEvent(AppDisplay *display, void *event);
extern int XSync(AppDisplay *display, int discard);
extern int XFlush(AppDisplay *display);
extern unsigned long XInternAtom(AppDisplay *display, const char *name, int only_if_exists);
extern int XGetWMProtocols(AppDisplay *display, WindowId window, unsigned long **protocols, int *count);
extern int XSendEvent(AppDisplay *display, WindowId window, int propagate, long mask, void *event);
extern int (*XSetErrorHandler(int (*handler)(AppDisplay *, void *)))(AppDisplay *, void *);
extern int open(const char *path, int flags, ...);
extern long write(int fd, const void *buffer, unsigned long count);
extern int close(int fd);
struct app_pollfd { int fd; short events, revents; };
extern int poll(struct app_pollfd *fds, unsigned long count, int timeout);
struct app_timespec { long seconds, nanoseconds; };
extern int clock_gettime(int clock, struct app_timespec *time);

#ifdef APP_OBSERVER_STEADY_RTT
#include "desktop-x11-latency.h"
#endif

static unsigned long length(const char *text) {
    unsigned long count = 0;
    while (text[count] != 0) {
        ++count;
    }
    return count;
}
static int lower(int byte) {
    if (byte >= 'A' && byte <= 'Z') {
        return byte + 'a' - 'A';
    }
    return byte;
}
static int contains(const char *text, const char *part) {
    for (; *text != 0; ++text) {
        unsigned long i = 0;
        while (part[i] != 0 && text[i] != 0 && lower(text[i]) == part[i]) {
            ++i;
        }
        if (part[i] == 0) {
            return 1;
        }
    }
    return 0;
}
static int equal(const char *left, const char *right) {
    unsigned long i = 0;
    while (left[i] != 0 && left[i] == right[i]) {
        ++i;
    }
    return left[i] == right[i];
}
static int title_matches(const char *title, const char *label) {
    if (equal(label, "terminal")) {
        return equal(title, "Terminal");
    }
    if (equal(label, "files")) {
        return contains(title, "xfe") || contains(title, "file explorer");
    }
    /* The stock Tiny Core editor starts with this document title, not its
     * application name. Initial desktop windows never have that title. */
    return equal(title, "Untitled") || contains(title, "editor");
}
static int ignore_destroyed_window(AppDisplay *display, void *event) {
    (void)display;
    (void)event;
    return 0;
}
static WindowId candidate, observer_root;
#ifdef APP_OBSERVER_TRACE_FAILURE
#include "desktop-observer-trace.h"
#endif
static void watch_window(AppDisplay *display, WindowId window) {
#ifdef APP_OBSERVER_TRACE_FAILURE
    ++observer_trace.watches;
#endif
    long mask = (1L << 17) | (1L << 22); /* StructureNotify | PropertyChange. */
    if (window == observer_root) {
        mask |= 1L << 19; /* Only root needs SubstructureNotify. */
    }
    XSelectInput(display, window, mask);
}
static int candidate_visible(AppDisplay *display, const char *label) {
#ifdef APP_OBSERVER_TRACE_FAILURE
    ++observer_trace.candidate_checks;
#endif
    char *title = 0;
    int matched = 0;
    if (XFetchName(display, candidate, &title) != 0 && title != 0) {
        matched = title_matches(title, label);
        XFree(title);
    }
    if (!matched) {
#ifdef APP_OBSERVER_TRACE_FAILURE
        trace_reset(1);
#endif
        candidate = 0;
        return 0;
    }
    struct app_window_attributes attributes;
    if (XGetWindowAttributes(display, candidate, &attributes) == 0) {
#ifdef APP_OBSERVER_TRACE_FAILURE
        trace_reset(2);
#endif
        candidate = 0;
        return 0;
    }
    return attributes.map_state == 2 && attributes.width > 0 && attributes.height > 0;
}
static int visible(AppDisplay *display, WindowId window, const char *label, int depth) {
    if (depth > 4) {
        return 0;
    }
    /* Observe creation/reparenting/mapping and late WM_NAME changes, without
     * requesting redirect authority or modifying any window. */
    watch_window(display, window);
    char *title = 0;
    if (XFetchName(display, window, &title) != 0 && title != 0) {
        int matched = title_matches(title, label);
        XFree(title);
        if (matched) {
            // Do not walk this client's widgets while waiting for its Map.
            candidate = window;
            return candidate_visible(display, label);
        }
    }
    WindowId root, parent, *children = 0;
    unsigned int count = 0;
    if (XQueryTree(display, window, &root, &parent, &children, &count) == 0) {
        return 0;
    }
    int found = 0;
    for (unsigned int i = 0; i < count && !found; ++i) {
        found = visible(display, children[i], label, depth + 1);
    }
    if (children != 0) {
        XFree(children);
    }
    return found;
}
static int report(int fd, const char *prefix, const char *label) {
    char output[96];
    unsigned long used = 0;
    for (unsigned long i = 0; i < length(prefix); ++i) {
        output[used++] = prefix[i];
    }
    for (unsigned long i = 0; i < length(label); ++i) {
        output[used++] = label[i];
    }
    output[used++] = '\n';
    return write(fd, output, used) == (long)used;
}
static int ping_reply(const union app_event *event, unsigned long protocols,
                      unsigned long ping, unsigned long nonce, WindowId window) {
    return event->client.type == 33 && event->client.format == 32
        && event->client.window == observer_root && event->client.message_type == protocols
        && (unsigned long)event->client.data[0] == ping
        && ((unsigned long)event->client.data[1] & 0xffffffffUL) == nonce
        && ((unsigned long)event->client.data[2] & 0xffffffffUL) == window;
}
static int report_ping(int output, const char *label, unsigned long nanos) {
    char line[160], digits[24];
    unsigned long used = 0, count = 0;
    const char *parts[] = {"APP-EVENTLOOP-REPLY ", label, " ping-roundtrip-ns="};
    for (unsigned long part = 0; part < 3; ++part) {
        for (unsigned long i = 0; parts[part][i] != 0; ++i) {
            line[used++] = parts[part][i];
        }
    }
    do {
        digits[count++] = '0' + nanos % 10;
        nanos /= 10;
    } while (nanos != 0);
    while (count != 0) {
        line[used++] = digits[--count];
    }
    line[used++] = '\n';
    return write(output, line, used) == (long)used;
}
/* EWMH response proves the stock client's event loop is running, not that
 * every pixel is painted. Unsupported clients fail, never silently pass. */
#ifdef APP_OBSERVER_SYSCALL_SCOPE
#include "desktop-syscall-scope.h"
#endif
static int await_ping(AppDisplay *display, int output, const char *label) {
    unsigned long protocols = XInternAtom(display, "WM_PROTOCOLS", 1);
    unsigned long ping = XInternAtom(display, "_NET_WM_PING", 1);
    unsigned long *advertised = 0;
    int count = 0, supported = 0;
    if (protocols == 0 || ping == 0
        || XGetWMProtocols(display, candidate, &advertised, &count) == 0) {
        return 10;
    }
    for (int i = 0; i < count; ++i) {
        supported |= advertised[i] == ping;
    }
    XFree(advertised);
    if (!supported) {
        return 10;
    }
    struct app_timespec begin, now;
    if (clock_gettime(1, &begin) != 0) {
        return 11;
    }
    unsigned long nonce = (unsigned long)begin.nanoseconds + 1;
    union app_event request = {0};
    request.client.type = 33;
    request.client.display = display;
    request.client.window = candidate;
    request.client.message_type = protocols;
    request.client.format = 32;
    request.client.data[0] = ping;
    request.client.data[1] = nonce;
    request.client.data[2] = candidate;
    if (XSendEvent(display, candidate, 0, 0, &request) == 0) {
        return 12;
    }
    XFlush(display);
    struct app_pollfd socket = {XConnectionNumber(display), 1, 0};
    for (;;) {
        if (XPending(display) == 0 && poll(&socket, 1, 1000) < 0) {
            return 13;
        }
        while (XPending(display) != 0) {
            union app_event event;
            XNextEvent(display, &event);
            if (ping_reply(&event, protocols, ping, nonce, candidate)) {
                if (clock_gettime(1, &now) != 0) {
                    return 11;
                }
                unsigned long nanos = (now.seconds - begin.seconds) * 1000000000UL
                    + now.nanoseconds - begin.nanoseconds;
#ifdef APP_OBSERVER_SYSCALL_SCOPE
                if (!app_scope_stop()) {
                    report(output, "APP-SCOPE-FAIL ", label);
                    return 16;
                }
#endif
                return report_ping(output, label, nanos) ? 0 : 14;
            }
        }
        if (clock_gettime(1, &now) != 0 || now.seconds - begin.seconds >= 90) {
            return 15;
        }
    }
}
int main(int argc, char **argv) {
#ifdef APP_OBSERVER_SYSCALL_SCOPE
    if (argc != 4 || !equal(argv[2], "ping")) {
        return 2;
    }
#else
    if ((argc != 2 && argc != 3)
        || (argc == 3 && !equal(argv[2], "ping"))) {
        return 2;
    }
#endif
    if (!equal(argv[1], "terminal") && !equal(argv[1], "files") && !equal(argv[1], "editor")) {
        return 2;
    }
    AppDisplay *display = XOpenDisplay("unix/:0");
    if (display == 0) {
        return 3;
    }
    XSetErrorHandler(ignore_destroyed_window);
    WindowId root = XDefaultRootWindow(display);
    observer_root = root;
    if (visible(display, root, argv[1], 0) || candidate != 0) {
        return 4; /* Never accept a stale, already visible matching window. */
    }
    XSync(display, 0);
    int output = open("/tmp/pointer-probe.log", 1 | 64 | 512, 0600);
#ifdef APP_OBSERVER_SYSCALL_SCOPE
    if (output < 0 || !app_scope_start(argv[3])) {
        if (output >= 0) {
            report(output, "APP-SCOPE-FAIL ", argv[1]);
        }
        return 17;
    }
#endif
    if (output < 0 || !report(output, "APP-OBSERVER-READY ", argv[1])) {
        return 5;
    }
    struct app_timespec begin, now;
    if (clock_gettime(1, &begin) != 0) {
#ifdef APP_OBSERVER_TRACE_FAILURE
        trace_failure(display, output, argv[1], "begin-clock", *__errno_location(), 0, 0);
#endif
        return 6;
    }
    struct app_pollfd socket = {XConnectionNumber(display), 1, 0};
    for (;;) {
        /* Event-driven observation; one-second fallback handles windows that
         * changed before their event subscription reached the X server. */
        if (XPending(display) == 0 && poll(&socket, 1, 1000) < 0) {
#ifdef APP_OBSERVER_TRACE_FAILURE
            trace_failure(display, output, argv[1], "discovery-poll", *__errno_location(), begin.seconds, 0);
#endif
            return 7;
        }
        int ready = 0;
        while (XPending(display) != 0 && !ready) {
            union app_event event;
            XNextEvent(display, &event);
#ifdef APP_OBSERVER_TRACE_FAILURE
            trace_xevent(&event);
#endif
            WindowId changed = 0;
            switch (event.structure.type) {
                case 16: /* CreateNotify: watch the new root child itself. */
                case 21: /* ReparentNotify: FLWM can move the client. */
                    changed = event.structure.window;
                    watch_window(display, changed);
                    break;
                case 19: /* MapNotify. */
                    changed = event.structure.window;
                    break;
                case 28: /* PropertyNotify, XA_WM_NAME. */
                    if (event.property.atom == 39) {
                        changed = event.property.window;
                    }
                    break;
                default:
                    break;
            }
            if (changed != 0) {
                char *title = 0;
                if (XFetchName(display, changed, &title) != 0 && title != 0) {
                    if (title_matches(title, argv[1])) {
                        candidate = changed;
#ifdef APP_OBSERVER_TRACE_FAILURE
                        observer_trace.last_match = changed;
#endif
                    }
                    XFree(title);
                }
                if (candidate != 0) {
                    ready = candidate_visible(display, argv[1]);
                }
            }
        }
        // A Map can race subscription. Check only the known candidate on a
        // timeout; never rescan another client's whole widget hierarchy.
        if (!ready && candidate != 0) {
            ready = candidate_visible(display, argv[1]);
        }
        if (ready) {
            if (argc >= 3) {
                int result = await_ping(display, output, argv[1]);
#ifdef APP_OBSERVER_STEADY_RTT
                if (result == 0) {
                    result = steady_roundtrip(display, root, output);
                    if (result != 0) {
                        report(output, "APP-X11-RTT-FAIL ", argv[1]);
                    }
                }
#endif
#ifdef APP_OBSERVER_SYSCALL_SCOPE
                if (result == 0 && !app_scope_save(output, argv[1])) {
                    report(output, "APP-SCOPE-FAIL ", argv[1]);
                    result = 18;
                }
#endif
                close(output);
                XCloseDisplay(display);
                return result;
            }
            if (!report(output, "APP-WINDOW-READY ", argv[1])) {
                return 8;
            }
            close(output);
            XCloseDisplay(display);
            return 0;
        }
#ifdef APP_OBSERVER_TRACE_FAILURE
        int sampled = clock_gettime(1, &now);
        if (sampled != 0) {
            trace_failure(display, output, argv[1], "discovery-clock", *__errno_location(), begin.seconds, 0);
            return 9;
        }
        if (now.seconds - begin.seconds >= 90) {
            trace_failure(display, output, argv[1], "discovery-deadline", 0, begin.seconds, now.seconds);
            return 9;
        }
#else
        if (clock_gettime(1, &now) != 0 || now.seconds - begin.seconds >= 90) {
            return 9;
        }
#endif
    }
}
#endif
