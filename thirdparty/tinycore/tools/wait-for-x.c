/* Profile glue, not a replacement X library: retry a real Xlib connection in
 * one process. These declarations use the existing Tiny Core LP64 libc/Xlib
 * ABI so assembling the runtime rootfs does not require development headers. */
typedef struct _XDisplay Display;
extern Display *XOpenDisplay(const char *name);
extern int XCloseDisplay(Display *display);
extern int kill(int pid, int signal);
struct wait_delay { long seconds, nanoseconds; };
extern int nanosleep(const struct wait_delay *delay, struct wait_delay *remaining);
extern int *__errno_location(void);

static int server_pid(const char *text) {
    int pid = 0;
    if (*text == '\0') {
        return 0;
    }
    for (; *text != '\0'; ++text) {
        if (*text < '0' || *text > '9' || pid > (2147483647 - (*text - '0')) / 10) {
            return 0;
        }
        pid = pid * 10 + (*text - '0');
    }
    return pid;
}

int main(int argc, char **argv) {
    if (argc != 3 || argv[1][0] == '\0') {
        return 2;
    }
    int pid = server_pid(argv[2]);
    if (pid == 0) {
        return 2;
    }
    for (int attempt = 0; attempt < 30; ++attempt) {
        Display *display = XOpenDisplay(argv[1]);
        if (display != 0) {
            /* XCloseDisplay performs a final XSync: success is a real server
             * round trip, not merely an existing pathname/listening socket. */
            XCloseDisplay(display);
            return 0;
        }
        if (attempt == 29 || kill(pid, 0) != 0) {
            return 1;
        }
        struct wait_delay remaining = {0, 100000000};
        while (nanosleep(&remaining, &remaining) != 0) {
            if (*__errno_location() != 4 || kill(pid, 0) != 0) {
                return 1;
            }
        }
    }
    return 1;
}
