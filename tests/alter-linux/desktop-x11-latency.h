#ifndef NANAMI_DESKTOP_X11_LATENCY_H
#define NANAMI_DESKTOP_X11_LATENCY_H

/* Opt-in diagnostic only. Never interpret this stack benchmark as pure IPC. */
extern int socket(int family, int kind, int protocol);
extern int connect(int fd, const void *address, unsigned int bytes);
extern long read(int fd, void *buffer, unsigned long bytes);
extern int nanosleep(const struct app_timespec *request, struct app_timespec *remaining);
extern int XQueryPointer(AppDisplay *display, WindowId window, WindowId *root,
                         WindowId *child, int *root_x, int *root_y,
                         int *window_x, int *window_y, unsigned int *mask);
#ifdef APP_OBSERVER_RTT_PHASES
extern int prctl(int option, ...);
#endif

enum { RTT_REQUESTS = 128, RTT_SAMPLES = 3, RTT_WARMUP = 8 };

/* Explicit test parameter, not a claim that any delay establishes quiescence. */
#ifndef APP_OBSERVER_RTT_SETTLE_SECONDS
#define APP_OBSERVER_RTT_SETTLE_SECONDS 2
#endif
#if APP_OBSERVER_RTT_SETTLE_SECONDS < 0 || APP_OBSERVER_RTT_SETTLE_SECONDS > 60
#error "observer settle must be between zero and sixty seconds"
#endif

static unsigned long rtt_now(void) {
    struct app_timespec time;
    if (clock_gettime(1, &time) != 0) {
        return 0;
    }
    return (unsigned long)time.seconds * 1000000000UL + time.nanoseconds;
}

static int rtt_transfer(int fd, unsigned char *bytes, unsigned long count, int sending) {
    while (count != 0) {
        long transferred;
        if (sending) {
            transferred = write(fd, bytes, count);
        } else {
            transferred = read(fd, bytes, count);
        }
        if (transferred <= 0 || (unsigned long)transferred > count) {
            return 0;
        }
        bytes += transferred;
        count -= transferred;
    }
    return 1;
}

static unsigned long rtt_u16(const unsigned char *bytes) {
    return bytes[0] | ((unsigned long)bytes[1] << 8);
}

static int rtt_connect(unsigned char request[8]) {
    unsigned char address[20] = {
        1, 0, '/', 't', 'm', 'p', '/', '.', 'X', '1', '1', '-', 'u', 'n', 'i', 'x', '/', 'X', '0', 0
    };
    int fd = socket(1, 1, 0);
    if (fd < 0) {
        return -1;
    }
    unsigned char setup[12] = {'l', 0, 11};
    unsigned char header[8], body[65536];
    if (connect(fd, address, sizeof(address)) != 0
        || !rtt_transfer(fd, setup, sizeof(setup), 1)
        || !rtt_transfer(fd, header, sizeof(header), 0) || header[0] != 1) {
        close(fd);
        return -1;
    }
    unsigned long bytes = rtt_u16(header + 6) * 4;
    if (bytes < 32 || bytes > sizeof(body) || !rtt_transfer(fd, body, bytes, 0)) {
        close(fd);
        return -1;
    }
    unsigned long root = 32 + ((rtt_u16(body + 16) + 3) & ~3UL) + 8UL * body[21];
    if (body[20] == 0 || root + 4 > bytes) {
        close(fd);
        return -1;
    }
    request[0] = 38; /* QueryPointer: fixed8-byte request and32-byte reply. */
    request[1] = 0;
    request[2] = 2;
    request[3] = 0;
    for (unsigned long index = 0; index < 4; ++index) {
        request[4 + index] = body[root + index];
    }
    return fd;
}

static void rtt_text(char *line, unsigned long *used, const char *text) {
    while (*text != 0) {
        line[(*used)++] = *text++;
    }
}

static void rtt_number(char *line, unsigned long *used, unsigned long value) {
    char digits[24];
    unsigned long count = 0;
    do {
        digits[count++] = '0' + value % 10;
        value /= 10;
    } while (value != 0);
    while (count != 0) {
        line[(*used)++] = digits[--count];
    }
}

static int steady_roundtrip(AppDisplay *display, WindowId root, int output) {
    struct app_timespec settle = {APP_OBSERVER_RTT_SETTLE_SECONDS, 0};
    if (nanosleep(&settle, 0) != 0) {
        return 20;
    }
    unsigned char request[8];
    int fd = rtt_connect(request);
    if (fd < 0) {
        return 21;
    }
    unsigned long costs[2][RTT_SAMPLES];
    unsigned int sequence = 0;
    int failed = 0;
    for (int mode = 0; mode < 2 && !failed; ++mode) {
#ifdef APP_OBSERVER_RTT_PHASES
        const char *name = "x11.raw";
        if (mode != 0) {
            name = "x11.xlib";
        }
        if (prctl(15, name, 0UL, 0UL, 0UL) != 0) {
            close(fd);
            return 24;
        }
#endif
        for (int sample = 0; sample < RTT_SAMPLES && !failed; ++sample) {
            unsigned long start = 0;
            for (int index = 0; index < RTT_WARMUP + RTT_REQUESTS; ++index) {
                if (index == RTT_WARMUP) {
                    start = rtt_now();
                }
                if (mode == 0) {
                    unsigned char reply[32];
                    ++sequence;
                    if (!rtt_transfer(fd, request, sizeof(request), 1)
                        || !rtt_transfer(fd, reply, sizeof(reply), 0)
                        || reply[0] != 1 || rtt_u16(reply + 2) != sequence
                        || reply[4] != 0 || reply[5] != 0 || reply[6] != 0 || reply[7] != 0) {
                        failed = 1;
                        break;
                    }
                } else {
                    WindowId returned_root, child;
                    int root_x, root_y, window_x, window_y;
                    unsigned int mask;
                    if (XQueryPointer(display, root, &returned_root, &child,
                                      &root_x, &root_y, &window_x, &window_y, &mask) == 0) {
                        failed = 1;
                        break;
                    }
                }
            }
            unsigned long end = rtt_now();
            if (start == 0 || end <= start) {
                failed = 1;
            }
            costs[mode][sample] = end - start;
        }
    }
#ifdef APP_OBSERVER_RTT_PHASES
    if (prctl(15, "x11.done", 0UL, 0UL, 0UL) != 0) {
        close(fd);
        return 24;
    }
#endif
    close(fd);
    if (failed) {
        return 22;
    }
    /* Keep every per-boot sample together in one complete result line. */
    char line[384];
    unsigned long used = 0;
    rtt_text(line, &used, "APP-X11-RTT-PASS count=128 raw-ns=");
    for (int mode = 0; mode < 2; ++mode) {
        if (mode != 0) {
            rtt_text(line, &used, " xlib-ns=");
        }
        for (int sample = 0; sample < RTT_SAMPLES; ++sample) {
            if (sample != 0) {
                line[used++] = ',';
            }
            rtt_number(line, &used, costs[mode][sample]);
        }
    }
    line[used++] = '\n';
    return write(output, line, used) == (long)used ? 0 : 23;
}

#endif
