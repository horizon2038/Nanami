/* fb-size-test: run under alter -g [--fb-size WxH], with expected W H.
 * Expected 0 0 checks that a 4096x2160 request was clamped by Honoka.
 * Link at 0x400000 for Alter's existing fork ELF-header lookup.
 */
#include <errno.h>
#include <fcntl.h>
#include <linux/fb.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <unistd.h>

static int check(unsigned int width, unsigned int height)
{
    struct stat before, after;
    struct fb_fix_screeninfo fix;
    struct fb_var_screeninfo var;
    /* Also exercise stat before the lazy graphics session exists. */
    if (stat("/dev/fb0", &before)) return 10;
    int fd = open("/dev/fb0", O_RDWR);
    if (fd < 0) return 11;
    for (int round = 0; round < 2; ++round) {
        if (ioctl(fd, FBIOGET_FSCREENINFO, &fix) || ioctl(fd, FBIOGET_VSCREENINFO, &var)) return 12;
        if (width && (var.xres != width || var.yres != height)) return 13;
        if (!width && (!var.xres || !var.yres || var.xres >= 4096 || var.yres >= 2160)) return 14;
        if (var.xres_virtual != var.xres || var.yres_virtual != var.yres ||
            var.bits_per_pixel != 32 || fix.line_length != var.xres * 4 ||
            fix.smem_len != fix.line_length * var.yres) return 15;
        if (fstat(fd, &after) || after.st_size != fix.smem_len || before.st_size != after.st_size) return 16;
        if (lseek(fd, 0, SEEK_END) != fix.smem_len) return 17;
        size_t rounded = (fix.smem_len + 4095UL) & ~4095UL;
        if (mmap(0, rounded + 4096, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0) != MAP_FAILED || errno != EINVAL) return 18;
        volatile uint32_t *pixels = mmap(0, fix.smem_len, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
        if (pixels == MAP_FAILED) return 19;
        pixels[0] = 0xff123456;
        uint32_t color = 0xffabcdef, readback = 0;
        if (lseek(fd, fix.smem_len - 4, SEEK_SET) != fix.smem_len - 4 || write(fd, &color, 4) != 4) return 20;
        if (pixels[fix.smem_len / 4 - 1] != color || pixels[0] != 0xff123456) return 21;
        if (read(fd, &readback, 4) != 0) return 22;
        if (lseek(fd, 0, SEEK_SET) != 0 || read(fd, &readback, 4) != 4 || readback != 0xff123456) return 23;
        if (munmap((void *)pixels, fix.smem_len)) return 24;
    }
    return close(fd) ? 25 : 0;
}

int main(int argc, char **argv)
{
    if (argc < 3) return 1;
    unsigned int width = strtoul(argv[1], 0, 10), height = strtoul(argv[2], 0, 10);
    if (argc == 4 && !strcmp(argv[3], "stress")) {
        /* 128 mmap/munmap pairs in the same guest/window owner. */
        for (int round = 0; round < 64; ++round) {
            int result = check(width, height);
            if (result) return result;
        }
        return 0;
    }
    if (argc == 4) return check(width, height);
    /* Fork/exec BEFORE opening fb0: the child must inherit the requested
     * size, not just an already-created session. Repeat once a session exists. */
    for (int round = 0; round < 2; ++round) {
        pid_t child = fork();
        if (child < 0) return 2;
        if (child == 0) {
            char *args[] = {argv[0], argv[1], argv[2], "child", 0};
            /* Alter intentionally supplies a basename as argv[0]. */
            execv("/bin/fb-size-test", args);
            _exit(3);
        }
        int status;
        if (waitpid(child, &status, 0) != child || !WIFEXITED(status) || WEXITSTATUS(status)) return 4;
        int result = check(width, height);
        if (result) return result;
    }
    return 0;
}
