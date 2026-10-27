/* Unix98 PTY integration: real Alter FDs, process groups, timers and epoll. */
typedef unsigned long word;
typedef unsigned int u32;
extern long linux_call(long, word, word, word, word, word, word);
extern void signal_restorer(void);
void *memset(void *pointer, int value, word bytes) {
    volatile unsigned char *out = pointer;
    for (word i = 0; i < bytes; ++i) { out[i] = (unsigned char)value; }
    return pointer;
}
static long call(long n, word a, word b, word c, word d) { return linux_call(n, a, b, c, d, 0, 0); }
static long io(long fd, word request, void *value) { return call(16, fd, request, (word)value, 0); }
static long put(long fd, const char *bytes, word len) { return call(1, fd, (word)bytes, len, 0); }
static long get(long fd, void *bytes, word len) { return call(0, fd, (word)bytes, len, 0); }
static void close_fd(long fd) { call(3, fd, 0, 0, 0); }
static void exit_child(int value) { call(60, value, 0, 0, 0); }
static int wait_child(long pid) {
    u32 status = ~0u;
    return pid > 0 && call(61, pid, (word)&status, 0, 0) == pid && status == 0;
}
static void delay(void) { word time[2] = {0, 50000000}; call(35, (word)time, 0, 0, 0); }
static int equal(const char *a, const char *b, word n) {
    for (word i = 0; i < n; ++i) { if (a[i] != b[i]) { return 0; } }
    return 1;
}
static int pair(long *master, long *slave) {
    *master = call(2, (word)"/dev/ptmx", 2 | 0x100, 0, 0);
    if (*master < 0) { return 0; }
    int locked = 0;
    if (io(*master, 0x40045431, &locked) != 0) { return 0; }
    *slave = call(16, *master, 0x5441, 2 | 0x100, 0);
    return *slave >= 0;
}
static int set_raw(long fd, unsigned char *termios, int minimum, int time) {
    if (io(fd, 0x5401, termios) != 0) { return 0; }
    for (int i = 0; i < 8; ++i) { termios[i] = 0; }
    for (int i = 12; i < 16; ++i) { termios[i] = 0; }
    termios[22] = (unsigned char)time; termios[23] = (unsigned char)minimum;
    return io(fd, 0x5402, termios) == 0;
}
static int basics(void) {
    long master = call(2, (word)"/dev/ptmx", 2 | 0x800 | 0x80000, 0, 0);
    u32 number = ~0u; int value = -1; char bytes[64]; unsigned char termios[36];
    if (master < 0 || io(master, 0x80045430, &number) != 0 || number != 0) { return 10; }
    if (io(master, (word)(long)(int)0x80045430, &number) != 0 || number != 0) { return 10; }
    if (io(master, 0x80045439, &value) != 0 || value != 1) { return 11; }
    if (call(2, (word)"/dev/pts/0", 2 | 0x100, 0, 0) != -5) { return 12; }
    value = 0;
    if (io(master, 0x40045431, &value) != 0) { return 13; }
    long slave = call(2, (word)"/dev/pts/0", 2 | 0x800 | 0x100, 0, 0);
    if (slave < 0 || get(master, bytes, 64) != -11) { return 14; }
    if (put(master, "abc", 3) != 3 || get(slave, bytes, 64) != -11) { return 15; }
    if (put(master, "\r", 1) != 1 || get(slave, bytes, 64) != 4 || !equal(bytes, "abc\n", 4)) { return 16; }
    if (get(master, bytes, 64) != 5 || !equal(bytes, "abc\r\n", 5)) { return 17; }
    if (put(master, "\4", 1) != 1 || get(slave, bytes, 64) != 0 || get(slave, bytes, 64) != -11) { return 18; }
    if (put(master, "\4", 1) != 1 || get(slave, bytes, 0) != 0 || get(slave, bytes, 64) != 0) { return 18; }
    long alias = call(32, master, 0, 0, 0);
    if (alias < 0 || call(72, alias, 4, 2, 0) != 0 || call(72, master, 3, 0, 0) != 2 || call(72, master, 1, 0, 0) != 1) { return 19; }
    call(72, master, 4, 0x802, 0);
    if (!set_raw(slave, termios, 1, 0)) { return 20; }
    if (put(master, "\0\r\3\377", 4) != 4) { return 21; }
    if (get(slave, 0, 4) != -14 || get(slave, bytes, 64) != 4 || !equal(bytes, "\0\r\3\377", 4)) { return 22; }
    if (put(slave, "output", 6) != 6) { return 23; }
    struct { int fd; short events, revents; } poll = { (int)master, 1, 0 };
    if (call(7, (word)&poll, 1, 0, 0) != 1 || !(poll.revents & 1)) { return 24; }
    word event[2] = {1 | ((word)77 << 32), 0}, observed[2] = {0};
    long epoll = call(291, 0, 0, 0, 0);
    if (epoll < 0 || call(233, epoll, 1, master, (word)event) != 0 || call(232, epoll, (word)observed, 1, 0) != 1) { return 25; }
    if (get(master, bytes, 64) != 6 || !equal(bytes, "output", 6)) { return 26; }
    close_fd(epoll);
    /* readv must read a single canonical record across all iovecs. */
    if (io(master, 0x5401, termios) != 0) { return 27; }
    termios[12] = 2;
    if (io(master, 0x5402, termios) != 0 || put(master, "abcd\nnext\n", 10) != 10) { return 28; }
    word iov[4] = {(word)bytes, 2, (word)(bytes + 2), 62};
    if (call(19, slave, (word)iov, 2, 0) != 5 || !equal(bytes, "abcd\n", 5) || get(slave, bytes, 64) != 5 || !equal(bytes, "next\n", 5)) { return 29; }
    close_fd(master);
    if (put(slave, "last", 4) != 4) { return 30; }
    close_fd(alias);
    if (get(slave, bytes, 64) != 0 || put(slave, "x", 1) != -5) { return 31; }
    if (call(2, (word)"/dev/pts/0", 2, 0, 0) != -2) { return 32; }
    close_fd(slave);
    return 0;
}
static int waits(void) {
    long master, slave; unsigned char termios[36]; char bytes[16];
    if (!pair(&master, &slave) || !set_raw(slave, termios, 0, 1)) { return 40; }
    if (get(slave, bytes, 16) != 0) { return 41; } /* VTIME with no data. */
    if (!set_raw(slave, termios, 3, 1)) { return 42; }
    long child = call(57, 0, 0, 0, 0);
    if (child == 0) { delay(); put(master, "a", 1); delay(); put(master, "b", 1); exit_child(0); }
    if (get(slave, bytes, 16) != 2 || !equal(bytes, "ab", 2) || !wait_child(child)) { return 43; }
    if (!set_raw(slave, termios, 1, 0)) { return 44; }
    child = call(57, 0, 0, 0, 0);
    if (child == 0) { close_fd(master); exit_child(get(slave, bytes, 16) == 0 ? 0 : 1); }
    close_fd(slave); delay(); close_fd(master);
    if (!wait_child(child)) { return 45; } /* Final master close wakes a reader. */
    if (!pair(&master, &slave) || !set_raw(slave, termios, 1, 0)) { return 46; }
    child = call(57, 0, 0, 0, 0);
    if (child == 0) { close_fd(master); delay(); put(slave, "delayed", 7); close_fd(slave); exit_child(0); }
    close_fd(slave);
    if (get(master, bytes, 16) != 7 || !equal(bytes, "delayed", 7) || !wait_child(child) || get(master, bytes, 16) != -5) { return 47; }
    close_fd(master);
    return 0;
}
static volatile int interrupted, resized, hungup;
static void handler(int signal) {
    if (signal == 2) { interrupted++; }
    if (signal == 28) { resized++; }
    if (signal == 1) { hungup++; }
}
static int signals(void) {
    long master, slave; int pipes[2];
    if (!pair(&master, &slave) || call(22, (word)pipes, 0, 0, 0) != 0) { return 50; }
    long child = call(57, 0, 0, 0, 0);
    if (child == 0) {
        close_fd(master); close_fd(pipes[0]);
        long pid = call(39, 0, 0, 0, 0);
        word action[4] = {(word)handler, 0x04000000, (word)signal_restorer, 0};
        if (call(112, 0, 0, 0, 0) != pid || call(16, slave, 0x540e, 0, 0) != 0) { exit_child(2); }
        if (call(13, 2, (word)action, 0, 8) != 0 || call(13, 28, (word)action, 0, 8) != 0 || call(13, 1, (word)action, 0, 8) != 0) { exit_child(3); }
        int foreground = 0;
        if (io(slave, 0x540f, &foreground) != 0 || foreground != pid) { exit_child(4); }
        long tty = call(2, (word)"/dev/tty", 2 | 0x100, 0, 0);
        if (tty < 0 || put(tty, "ctty", 4) != 4) { exit_child(5); }
        close_fd(tty);
        for (int fd = 0; fd < 3; ++fd) { if (call(33, slave, fd, 0, 0) != fd) { exit_child(6); } }
        close_fd(slave);
        put(pipes[1], "r", 1);
        char bytes[8];
        if (get(0, bytes, 8) != -4 || interrupted != 1) { exit_child(7); }
        put(pipes[1], "i", 1);
        if (get(0, bytes, 8) != -4 || resized != 1) { exit_child(8); }
        put(pipes[1], "w", 1);
        get(0, bytes, 8); /* SIGHUP can interrupt or race with the EOF return. */
        if (get(0, bytes, 8) != 0 || hungup != 1) { exit_child(9); }
        exit_child(0);
    }
    close_fd(slave); close_fd(pipes[1]); char byte, output[8];
    if (child < 0 || get(pipes[0], &byte, 1) != 1 || get(master, output, 8) != 4 || !equal(output, "ctty", 4)) { return 51; }
    delay();
    if (put(master, "\3", 1) != 1 || get(pipes[0], &byte, 1) != 1 || byte != 'i') { return 52; }
    delay(); unsigned short size[4] = {27, 91, 0, 0};
    if (io(master, 0x5414, size) != 0 || get(pipes[0], &byte, 1) != 1 || byte != 'w') { return 53; }
    delay(); close_fd(master);
    if (!wait_child(child)) { return 54; }
    close_fd(pipes[0]);
    return 0;
}
static int job_control(void) {
    long master, slave;
    if (!pair(&master, &slave)) { return 70; }
    long owner = call(57, 0, 0, 0, 0);
    if (owner == 0) {
        if (call(112, 0, 0, 0, 0) < 0 || call(16, slave, 0x540e, 0, 0) != 0) { exit_child(1); }
        /* A background read stops even with O_NONBLOCK. */
        call(72, slave, 4, 0x802, 0);
        long reader = call(57, 0, 0, 0, 0);
        if (reader == 0) {
            char bytes[4];
            if (call(109, 0, 0, 0, 0) != 0) { exit_child(2); }
            exit_child(get(slave, bytes, 4) == 2 && equal(bytes, "x\n", 2) ? 0 : 3);
        }
        u32 status = 0;
        if (reader < 0 || call(61, reader, (word)&status, 2, 0) != reader || status != (21u << 8 | 127)) { exit_child(4); }
        int group = (int)reader;
        if (io(slave, 0x5410, &group) != 0 || put(master, "x\n", 2) != 2 || call(62, reader, 18, 0, 0) != 0 || !wait_child(reader)) { exit_child(5); }
        /* The shell is now background; ignoring SIGTTOU lets it regain tty. */
        word ignore[4] = {1, 0, 0, 0};
        if (call(13, 22, (word)ignore, 0, 8) != 0) { exit_child(6); }
        group = (int)call(39, 0, 0, 0, 0);
        if (io(slave, 0x5410, &group) != 0) { exit_child(7); }
        exit_child(0);
    }
    close_fd(slave);
    if (!wait_child(owner)) { return 71; }
    close_fd(master);
    return 0;
}
static int drains_and_rights(void) {
    long master, slave; unsigned char termios[36]; char bytes[64];
    if (!pair(&master, &slave) || !set_raw(slave, termios, 1, 0)) { return 80; }
    static char fill[4096];
    memset(fill, 'x', sizeof(fill));
    call(72, master, 4, 0x802, 0); call(72, slave, 4, 0x802, 0);
    for (int i = 0; i < 16; ++i) {
        long written = put(slave, fill, sizeof(fill));
        if (written < 0) { return 88; }
        if (written < (long)sizeof(fill)) { break; }
    }
    struct { int fd; short events, revents; } poll = {(int)master, 4, 0};
    if (put(slave, "x", 1) != -11 || call(7, (word)&poll, 1, 0, 0) != 1 || !(poll.revents & 4)) { return 89; }
    call(72, slave, 4, 2, 0);
    long child = call(57, 0, 0, 0, 0);
    if (child == 0) { delay(); exit_child(get(master, bytes, sizeof(bytes)) == sizeof(bytes) ? 0 : 1); }
    if (put(slave, "x", 1) != 1 || !wait_child(child)) { return 90; }
    long drained;
    do { drained = get(master, fill, sizeof(fill)); } while (drained > 0);
    if (drained != -11) { return 91; }
    call(72, master, 4, 2, 0);
    if (put(slave, "drain", 5) != 5) { return 92; }
    child = call(57, 0, 0, 0, 0);
    if (child == 0) { delay(); exit_child(get(master, bytes, 64) == 5 && equal(bytes, "drain", 5) ? 0 : 1); }
    termios[23] = 0;
    if (io(slave, 0x5403, termios) != 0 || !wait_child(child)) { return 81; }
    int sockets[2];
    if (call(53, 1, 1, 0, (word)sockets) != 0) { return 82; }
    char byte = 'f'; word iov[2] = {(word)&byte, 1};
    word control[3] = {20, ((word)1 << 32) | 1, (word)master};
    word message[7] = {0, 0, (word)iov, 1, (word)control, 24, 0};
    if (call(46, sockets[0], (word)message, 0, 0) != 1) { return 83; }
    close_fd(master);
    /* Queued rights own the master even before recvmsg installs its new FD. */
    if (put(slave, "owned", 5) != 5) { return 84; }
    control[0] = 0; control[1] = 0; control[2] = 0;
    if (call(47, sockets[1], (word)message, 0, 0) != 1) { return 85; }
    master = (u32)control[2];
    if (get(master, bytes, 64) != 5 || !equal(bytes, "owned", 5)) { return 86; }
    close_fd(master);
    if (get(slave, bytes, 64) != 0) { return 87; }
    close_fd(slave); close_fd(sockets[0]); close_fd(sockets[1]);
    return 0;
}
static const char script[] =
    "local function has(line)\n"
    "  for _,s in ipairs(vim.api.nvim_buf_get_lines(0,0,-1,false)) do if s == line then return true end end\n"
    "  return false\nend\n"
    "local function check(ok, text) if not ok then error(text) end end\n"
    "vim.cmd('terminal /bin/busybox sh -i')\n"
    "local job = vim.b.terminal_job_id\n"
    "check(job and job > 0, 'forkpty failed')\n"
    "vim.fn.chansend(job, \"printf 'PTY-%s\\\\n' READY\\n\")\n"
    "check(vim.wait(5000,function() return has('PTY-READY') end,10), 'shell output')\n"
    "vim.fn.chansend(job, '/bin/busybox sleep 30\\n')\n"
    "vim.wait(200,function() return false end,10)\n"
    "vim.fn.chansend(job, string.char(3))\n"
    "vim.fn.chansend(job, \"printf 'PTY-%s\\\\n' INTERRUPTED\\n\")\n"
    "check(vim.wait(5000,function() return has('PTY-INTERRUPTED') end,10), 'Ctrl-C')\n"
    "vim.fn.jobresize(job,91,27)\n"
    "vim.fn.chansend(job, '/bin/busybox stty size\\n')\n"
    "check(vim.wait(5000,function() return has('27 91') end,10), 'resize')\n"
    "vim.fn.chansend(job, 'exit 0\\n')\n"
    "check(vim.fn.jobwait({job},5000)[1] == 0, 'shell exit')\n"
    "vim.fn.writefile({'nvim-pty-ok'},'/tmp/nvim-pty-ok')\n"
    "vim.cmd('qa!')\n";
int test_main(void) {
    int result = basics(); if (result != 0) { return result; }
    result = waits(); if (result != 0) { return result; }
    result = signals(); if (result != 0) { return result; }
    result = job_control(); if (result != 0) { return result; }
    result = drains_and_rights(); if (result != 0) { return result; }
    long file = call(2, (word)"/tmp/nvim-pty.lua", 0x242, 0600, 0);
    if (file < 0 || put(file, script, sizeof(script) - 1) != sizeof(script) - 1) { return 60; }
    close_fd(file);
    /* A closed native stdio FD must not silently reappear as the GUI tty. */
    close_fd(0); char byte;
    if (get(0, &byte, 1) != -9) { return 61; }
    put(1, "pty-ok\n", 7);
    return 0;
}
