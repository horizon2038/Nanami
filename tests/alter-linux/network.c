/* Freestanding x86-64 Linux regression. Run on a QEMU snapshot with an isolated
 * user-mode NIC (10.0.2.0/24). BusyBox must be installed as /bin/busybox.
 * Link at 0x400000: Alter's existing static-fork path reads the ELF header there. */
static long sc(long n, long a, long b, long c) {
    long result;
    register long fourth __asm__("r10") = 0;
    __asm__ volatile("syscall" : "=a"(result) : "a"(n), "D"(a), "S"(b), "d"(c), "r"(fourth)
                     : "rcx", "r11", "memory");
    return result;
}
#define SC(n,a,b,c) sc(n,(long)(a),(long)(b),(long)(c))
#define CHECK(c) do { if (!(c)) return __LINE__; } while (0)
static char output[8192];
static int contains(const char *text) {
    for (int i = 0; output[i]; ++i) {
        int j = 0;
        while (text[j] && output[i+j] == text[j]) ++j;
        if (!text[j]) return 1;
    }
    return 0;
}
static int run(char **argv) {
    long child = SC(57, 0, 0, 0);
    if (child == 0) {
        long fd = SC(2, "/tmp/network-test-output", 2 | 64 | 512, 0600);
        if (fd < 0) SC(60, 120, 0, 0);
        SC(33, fd, 1, 0); SC(33, fd, 2, 0); SC(3, fd, 0, 0);
        char *env[] = { "PATH=/bin:/sbin:/usr/bin:/usr/sbin", "TERM=vt100", 0 };
        SC(59, "/bin/busybox", argv, env);
        SC(60, 121, 0, 0);
    }
    if (child < 0) return -1;
    int status = -1;
    if (SC(61, child, &status, 0) != child) return -2;
    long fd = SC(2, "/tmp/network-test-output", 0, 0);
    if (fd < 0) return -3;
    long bytes = SC(0, fd, output, sizeof(output) - 1);
    SC(3, fd, 0, 0);
    if (bytes < 0) return -4;
    output[bytes] = 0;
    SC(1, 1, output, bytes);
    return status;
}
struct stat64 {
    unsigned long dev, ino, nlink;
    unsigned int mode, uid, gid, pad;
    unsigned long rdev;
    long size, blksize, blocks, atime, atime_ns, mtime, mtime_ns, ctime, ctime_ns, reserved[3];
};
long test_main(void) {
    char *install[] = { "busybox", "--install", 0 };
    CHECK(run(install) == 0 && output[0] == 0);
    CHECK(run(install) == 0 && output[0] == 0); /* EEXIST is not EINVAL/EIO. */
    struct stat64 binary, arp, ip;
    CHECK(SC(4, "/bin/busybox", &binary, 0) == 0);
    CHECK(SC(4, "/sbin/arp", &arp, 0) == 0 && arp.ino == binary.ino);
    CHECK(SC(4, "/sbin/ip", &ip, 0) == 0 && ip.ino == binary.ino);
    CHECK(SC(86, "/bin/busybox", "/sbin/arp", 0) == -17);
    CHECK(SC(86, "/bin/busybox", "/missing-parent/arp", 0) == -2);
    CHECK(SC(86, "/missing-source", "/sbin/missing-link", 0) == -2);

    long socket = SC(41, 2, 2, 0);
    CHECK(socket >= 0);
    unsigned char ifreq[40] = { 'e','t','h','0' };
    CHECK(SC(16, socket, 0x8933, ifreq) == 0 && ifreq[16] == 2);
    CHECK(SC(16, socket, 0x8915, ifreq) == 0 && ifreq[16] == 2);
    CHECK(ifreq[20] == 10 && ifreq[21] == 0 && ifreq[22] == 2);
    CHECK(SC(16, socket, 0x8942, ifreq) == 0);
    CHECK(ifreq[16] == 0 && ifreq[17] == 0 && ifreq[18] == 0 && ifreq[19] == 0);
    struct { int len, padding; void *buffer; } conf = { 0, 0, 0 };
    CHECK(SC(16, socket, 0x8912, &conf) == 0 && conf.len == 80);
    unsigned char interfaces[80];
    conf.buffer = interfaces;
    CHECK(SC(16, socket, 0x8912, &conf) == 0 && conf.len == 80);
    CHECK(interfaces[0] == 'l' && interfaces[1] == 'o' && interfaces[40] == 'e');
    CHECK(SC(3, socket, 0, 0) == 0);

    long config = SC(2, "/etc/resolv.conf", 0, 0);
    CHECK(config >= 0);
    long count = SC(0, config, output, sizeof(output) - 1);
    CHECK(count > 0); output[count] = 0;
    CHECK(contains("nameserver 10.0.2.3\n"));
    CHECK(SC(3, config, 0, 0) == 0);
    CHECK(SC(4, "/etc/resolv.conf", &binary, 0) == 0 && binary.size == count);
    config = SC(2, "/etc/resolv.conf", 1 | 64 | 512, 0644);
    CHECK(config >= 0);
    const char custom[] = "nameserver 192.0.2.53\n";
    CHECK(SC(1, config, custom, sizeof(custom) - 1) == sizeof(custom) - 1);
    CHECK(SC(3, config, 0, 0) == 0);
    config = SC(2, "/etc/resolv.conf", 0, 0);
    CHECK(config >= 0);
    count = SC(0, config, output, sizeof(output) - 1);
    CHECK(count == sizeof(custom) - 1); output[count] = 0;
    CHECK(contains(custom));
    CHECK(SC(3, config, 0, 0) == 0 && SC(87, "/etc/resolv.conf", 0, 0) == 0);

    char *local[] = { "busybox", "ping", "-c", "1", "localhost", 0 };
    CHECK(run(local) == 0 && contains("1 packets received"));
    char *ping[] = { "busybox", "ping", "-c", "1", "10.0.2.2", 0 };
    CHECK(run(ping) == 0 && contains("1 packets received"));
    char *neighbors[] = { "busybox", "arp", "-n", 0 };
    CHECK(run(neighbors) == 0 && contains("10.0.2.2") && contains("eth0"));
    /* BusyBox reports a failed SIOCGIFTXQLEN but still exits successfully. */
    char *short_addresses[] = { "busybox", "ip", "a", 0 };
    CHECK(run(short_addresses) == 0 && contains("127.0.0.1") && contains("eth0"));
    CHECK(!contains("ioctl ") && !contains("Not a tty"));
    char *addresses[] = { "busybox", "ip", "addr", 0 };
    CHECK(run(addresses) == 0 && contains("127.0.0.1") && contains("eth0"));
    CHECK(!contains("ioctl ") && !contains("Not a tty"));
    char *links[] = { "busybox", "ip", "link", 0 };
    CHECK(run(links) == 0 && contains("lo:") && contains("eth0:"));
    CHECK(!contains("ioctl ") && !contains("Not a tty"));
    const char ok[] = "network fixture passed\n";
    SC(1, 1, ok, sizeof(ok) - 1);
    return 0;
}
__asm__(".global _start\n_start:\nxor %ebp,%ebp\nand $-16,%rsp\ncall test_main\nmov %rax,%rdi\nmov $60,%eax\nsyscall\nud2\n");
