#ifndef NANAMI_DESKTOP_OBSERVER_TRACE_H
#define NANAMI_DESKTOP_OBSERVER_TRACE_H

/* Opt-in, test-only failure evidence. Never discover a window on behalf of
 * the observer or turn a failed readiness test into success. */
extern int *__errno_location(void);
enum { TRACE_EVENTS = 16, TRACE_NODES = 24 };
struct observer_trace_event { int type; WindowId window; };
static struct {
    unsigned long events, watches, candidate_checks, resets;
    WindowId last_match, last_reset;
    unsigned long reset_reason;
    struct observer_trace_event recent[TRACE_EVENTS];
} observer_trace;

struct trace_line {
    char bytes[4096];
    unsigned long used;
    unsigned int nodes;
};
static void trace_text(struct trace_line *line, const char *text) {
    while (*text != 0 && line->used < sizeof(line->bytes) - 1) {
        line->bytes[line->used++] = *text++;
    }
}
static void trace_number(struct trace_line *line, unsigned long value) {
    char digits[24];
    unsigned long count = 0;
    do {
        digits[count++] = '0' + value % 10;
        value /= 10;
    } while (value != 0);
    while (count != 0 && line->used < sizeof(line->bytes) - 1) {
        line->bytes[line->used++] = digits[--count];
    }
}
static void trace_field(struct trace_line *line, const char *key, unsigned long value) {
    trace_text(line, key);
    trace_number(line, value);
}
static void trace_event(int type, WindowId window) {
    unsigned long index = observer_trace.events++ % TRACE_EVENTS;
    observer_trace.recent[index] = (struct observer_trace_event){type, window};
}
static void trace_xevent(const union app_event *event) {
    int type = event->structure.type;
    WindowId window = 0;
    if (type == 16 || type == 17 || type == 19 || type == 21 || type == 22) {
        window = event->structure.window;
    } else if (type == 28 || type == 33) {
        window = event->property.window;
    }
    trace_event(type, window);
}
static void trace_reset(unsigned long reason) {
    ++observer_trace.resets;
    observer_trace.last_reset = candidate;
    observer_trace.reset_reason = reason;
}

/* Failure-only snapshot: no subscriptions, candidate mutation or unbounded
 * traversal of a client's widget tree. Retain the original failure result. */
static void trace_tree(AppDisplay *display, WindowId window, int depth,
                       struct trace_line *line, const char *label) {
    if (depth > 2 || line->nodes == TRACE_NODES) {
        return;
    }
    ++line->nodes;
    trace_field(line, " [w=", window);
    trace_field(line, " d=", (unsigned long)depth);
    struct app_window_attributes attributes;
    if (XGetWindowAttributes(display, window, &attributes) != 0) {
        trace_field(line, " map=", (unsigned long)attributes.map_state);
        trace_field(line, " width=", (unsigned long)attributes.width);
        trace_field(line, " height=", (unsigned long)attributes.height);
    }
    char *title = 0;
    int matched = 0;
    if (XFetchName(display, window, &title) != 0 && title != 0) {
        matched = title_matches(title, label);
        trace_field(line, " match=", (unsigned long)matched);
        trace_text(line, " title=");
        for (unsigned int i = 0; i < 24 && title[i] != 0; ++i) {
            char escaped[2] = {'_', 0};
            if (title[i] > ' ' && title[i] < 127 && title[i] != '[' && title[i] != ']') {
                escaped[0] = title[i];
            }
            trace_text(line, escaped);
        }
        XFree(title);
    }
    trace_text(line, "]");
    if (matched || depth == 2 || line->nodes == TRACE_NODES) {
        return;
    }
    WindowId root, parent, *children = 0;
    unsigned int count = 0;
    if (XQueryTree(display, window, &root, &parent, &children, &count) == 0) {
        return;
    }
    for (unsigned int i = 0; i < count && line->nodes < TRACE_NODES; ++i) {
        trace_tree(display, children[i], depth + 1, line, label);
    }
    if (children != 0) {
        XFree(children);
    }
}

static void trace_failure(AppDisplay *display, int output, const char *label,
                          const char *stage, int error, long begin, long now) {
    struct trace_line line = {{0}, 0, 0};
    trace_text(&line, "APP-OBSERVER-FAIL stage=");
    trace_text(&line, stage);
    trace_field(&line, " errno=", (unsigned long)error);
    trace_field(&line, " begin-s=", (unsigned long)begin);
    trace_field(&line, " now-s=", (unsigned long)now);
    trace_field(&line, " candidate=", candidate);
    trace_field(&line, " events=", observer_trace.events);
    trace_field(&line, " watches=", observer_trace.watches);
    trace_field(&line, " checks=", observer_trace.candidate_checks);
    trace_field(&line, " last-match=", observer_trace.last_match);
    trace_field(&line, " resets=", observer_trace.resets);
    trace_field(&line, " last-reset=", observer_trace.last_reset);
    trace_field(&line, " reset-reason=", observer_trace.reset_reason);
    trace_text(&line, " recent=");
    unsigned long first = 0;
    if (observer_trace.events > TRACE_EVENTS) {
        first = observer_trace.events - TRACE_EVENTS;
    }
    for (unsigned long i = first; i < observer_trace.events; ++i) {
        struct observer_trace_event event = observer_trace.recent[i % TRACE_EVENTS];
        trace_number(&line, (unsigned long)event.type);
        trace_field(&line, ":", event.window);
        trace_text(&line, ",");
    }
    trace_text(&line, " tree:");
    trace_tree(display, observer_root, 0, &line, label);
    trace_field(&line, " nodes=", line.nodes);
    line.bytes[line.used++] = '\n';
    unsigned long done = 0;
    while (done < line.used) {
        long count = write(output, line.bytes + done, line.used - done);
        if (count <= 0 || (unsigned long)count > line.used - done) {
            break;
        }
        done += (unsigned long)count;
    }
}
#endif
