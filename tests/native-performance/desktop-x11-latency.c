#include <assert.h>
#include <limits.h>
#include <string.h>

typedef unsigned long WindowId;
typedef struct _XDisplay AppDisplay;
struct app_timespec { long seconds, nanoseconds; };
extern int clock_gettime(int clock, struct app_timespec *time);
extern int close(int fd);
extern long mock_read(int fd, void *buffer, unsigned long bytes);
extern long mock_write(int fd, const void *buffer, unsigned long bytes);
static long requested_settle = -1;
static int mock_nanosleep(const struct app_timespec *request, struct app_timespec *remaining) {
    assert(remaining == 0);
    assert(request->nanoseconds == 0);
    requested_settle = request->seconds;
    return -1;
}
#define read mock_read
#define write mock_write
#define nanosleep mock_nanosleep
#include "../alter-linux/desktop-x11-latency.h"
#undef read
#undef write
#undef nanosleep

static unsigned char payload[32];
static unsigned long position;
static int failure;

long mock_read(int fd, void *buffer, unsigned long bytes) {
    assert(fd == 7);
    if (failure == 1) {
        return 0;
    }
    if (failure == 2) {
        return (long)bytes + 1;
    }
    unsigned long copied = bytes;
    if (copied > 3) {
        copied = 3;
    }
    memcpy(buffer, payload + position, copied);
    position += copied;
    return (long)copied;
}

long mock_write(int fd, const void *buffer, unsigned long bytes) {
    assert(fd == 7);
    if (failure == 1) {
        return -1;
    }
    if (failure == 2) {
        return (long)bytes + 1;
    }
    unsigned long copied = bytes;
    if (copied > 3) {
        copied = 3;
    }
    memcpy(payload + position, buffer, copied);
    position += copied;
    return (long)copied;
}

int main(void) {
    /* Failed settle must stop before any X server or socket operation. */
    assert(steady_roundtrip(0, 0, -1) == 20);
    assert(requested_settle == APP_OBSERVER_RTT_SETTLE_SECONDS);
    const unsigned char little[] = {0x34, 0x12};
    assert(rtt_u16(little) == 0x1234);
    char line[384] = {0};
    unsigned long used = 0;
    rtt_text(line, &used, "sample=");
    rtt_number(line, &used, 0);
    rtt_text(line, &used, ",");
    rtt_number(line, &used, ULONG_MAX);
    assert(strcmp(line, "sample=0,18446744073709551615") == 0);
    unsigned char input[19], output[19];
    for (unsigned int index = 0; index < sizeof(input); ++index) {
        input[index] = (unsigned char)(index + 1);
    }
    assert(rtt_transfer(7, input, sizeof(input), 1));
    assert(position == sizeof(input));
    position = 0;
    assert(rtt_transfer(7, output, sizeof(output), 0));
    assert(position == sizeof(output));
    assert(memcmp(input, output, sizeof(input)) == 0);
    for (failure = 1; failure <= 2; ++failure) {
        assert(!rtt_transfer(7, input, sizeof(input), 1));
        assert(!rtt_transfer(7, output, sizeof(output), 0));
    }
    return 0;
}
