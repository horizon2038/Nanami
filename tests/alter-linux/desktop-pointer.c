/* Integer-only X11 QueryPointer probe. Observe the server's actual pointer,
 * not Honoka's cursor or an echoed copy of the injected input coordinates. */
typedef unsigned long word;
extern long linux_call(long, word, word, word, word, word, word);
static long call(long n, word a, word b, word c, word d) {
    return linux_call(n, a, b, c, d, 0, 0);
}
static int transfer(int fd, unsigned char *bytes, word count, int write) {
    while (count != 0) {
        long n = call(write ? 1 : 0, fd, (word)bytes, count, 0);
        if (n <= 0) { return 0; }
        bytes += n;
        count -= n;
    }
    return 1;
}
static unsigned short u16(const unsigned char *p) {
    return p[0] | ((unsigned short)p[1] << 8);
}
static word decimal(char *output, int value) {
    char digits[16];
    word count = 0;
    word used = 0;
    if (value < 0) { output[used++] = '-'; value = -value; }
    do { digits[count++] = '0' + value % 10; value /= 10; } while (value != 0);
    while (count != 0) { output[used++] = digits[--count]; }
    return used;
}
int test_main(void) {
    int fd = call(41, 1, 1, 0, 0);
    unsigned char address[22] = {1, 0, '/', 't', 'm', 'p', '/', '.', 'X', '1', '1', '-', 'u', 'n', 'i', 'x', '/', 'X', '0', 0};
    if (fd < 0 || call(42, fd, (word)address, 20, 0) != 0) { return 10; }
    unsigned char setup[12] = {'l', 0, 11};
    unsigned char header[8];
    unsigned char body[65536];
    if (!transfer(fd, setup, sizeof(setup), 1) || !transfer(fd, header, 8, 0) || header[0] != 1) { return 11; }
    word bytes = (word)u16(header + 6) * 4;
    if (bytes < 32 || bytes > sizeof(body) || !transfer(fd, body, bytes, 0)) { return 12; }
    word root = 32 + ((u16(body + 16) + 3) & ~3UL) + 8UL * body[21];
    if (body[20] == 0 || root + 4 > bytes) { return 13; }
    unsigned char request[8] = {38, 0, 2, 0};
    for (word i = 0; i < 4; ++i) { request[4 + i] = body[root + i]; }
    int log = call(2, (word)"/tmp/pointer-probe.log", 01001 | 0100, 0600, 0);
    if (log < 0) { return 14; }
    int previous_x = -10000;
    int previous_y = -10000;
    for (;;) {
        unsigned char reply[32];
        if (!transfer(fd, request, 8, 1) || !transfer(fd, reply, 32, 0) || reply[0] != 1) { return 15; }
        int x = (short)u16(reply + 16);
        int y = (short)u16(reply + 18);
        if (x != previous_x || y != previous_y) {
            char line[64] = "X11-POINTER ";
            word used = 12;
            used += decimal(line + used, x);
            line[used++] = ' ';
            used += decimal(line + used, y);
            line[used++] = '\n';
            if (!transfer(log, (unsigned char *)line, used, 1)) { return 16; }
            previous_x = x;
            previous_y = y;
        }
        word delay[2] = {0, 100000000};
        call(35, (word)delay, 0, 0, 0);
    }
}
