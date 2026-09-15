#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <unistd.h>

static __thread int tls_value = 37;
static char bss[8192];

#define CHECK(expr) do { if (!(expr)) { \
    fprintf(stderr, "FAIL line %d: %s (errno=%d)\n", __LINE__, #expr, errno); \
    return 1; } } while (0)

int main(int argc, char **argv) {
    CHECK(tls_value == 37 && bss[0] == 0 && bss[8191] == 0);
    if (argc == 2 && strcmp(argv[1], "exec-child") == 0) {
        CHECK(strcmp(argv[0], "preserved-argv-zero") == 0);
        puts("PASS dynamic execve, argv, TLS and BSS");
        return 0;
    }
    char *heap = malloc(131072);
    CHECK(heap != NULL);
    memset(heap, 0x53, 131072);
    CHECK(heap[131071] == 0x53);
    free(heap);
    void *library = dlopen("/lib/libalter-probe.so", RTLD_NOW | RTLD_LOCAL);
    CHECK(library != NULL);
    int (*probe)(void) = (int (*)(void))dlsym(library, "alter_probe");
    CHECK(probe && probe() == 42 && probe() == 43);
    CHECK(dlclose(library) == 0);
    puts("PASS dynamic relocations, malloc, dlopen/dlsym/dlclose and DSO TLS");

    const char *path = "/tmp/positioned-io-test";
    int fd = open(path, O_CREAT | O_TRUNC | O_RDWR | O_CLOEXEC, 0600);
    CHECK(fd >= 0);
    char *missing_args[] = {"missing", NULL};
    CHECK(execv("/bin/glibc-missing-interp", missing_args) == -1 && errno == ENOENT);
    CHECK((fcntl(fd, F_GETFL) & O_ACCMODE) == O_RDWR);
    CHECK(fcntl(fd, F_GETFD) == FD_CLOEXEC);
    CHECK(write(fd, "abcdef", 6) == 6);
    CHECK(lseek(fd, 2, SEEK_SET) == 2);
    int alias = dup(fd);
    CHECK(alias >= 0 && fcntl(alias, F_GETFD) == 0);
    CHECK(pwrite(alias, "XY", 2, 1) == 2);
    char data[8] = {0};
    CHECK(pread(fd, data, 6, 0) == 6 && memcmp(data, "aXYdef", 6) == 0);
    CHECK(lseek(fd, 0, SEEK_CUR) == 2 && lseek(alias, 0, SEEK_CUR) == 2);
    CHECK(pread(fd, data, 1, -1) == -1 && errno == EINVAL);
    struct stat st;
    CHECK(fstatat(fd, "", &st, AT_EMPTY_PATH) == 0 && st.st_size == 6);
    CHECK(fstatat(fd, "", &st, 0) == -1 && errno == ENOENT);
    char *map = mmap(NULL, 4096, PROT_READ | PROT_WRITE, MAP_PRIVATE, fd, 0);
    CHECK(map != MAP_FAILED && memcmp(map, "aXYdef", 6) == 0 && map[6] == 0);
    map[0] = 'z';
    CHECK(pread(fd, data, 1, 0) == 1 && data[0] == 'a');
    CHECK(lseek(fd, 0, SEEK_CUR) == 2);
    CHECK(munmap(map, 4096) == 0);
    CHECK(mmap(NULL, 4096, PROT_READ, MAP_PRIVATE, fd, 1) == MAP_FAILED && errno == EINVAL);
    CHECK(mmap(NULL, 4096, PROT_READ, MAP_PRIVATE, -1, 0) == MAP_FAILED && errno == EBADF);
    char *range = mmap(NULL, 3 * 4096, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    CHECK(range != MAP_FAILED);
    range[0] = 11;
    range[8192] = 22;
    CHECK(munmap(range + 4096, 4096) == 0);
    CHECK(mmap(range, 8192, PROT_READ, MAP_PRIVATE | MAP_FIXED, fd, 0) == range);
    CHECK(range[0] == 'a' && range[8192] == 22);
    CHECK(munmap(range + 4096, 4096) == 0);
    CHECK(munmap(range, 3 * 4096) == 0); /* Already-unmapped holes are legal. */
    CHECK(fcntl(alias, F_SETFL, O_APPEND) == 0);
    CHECK((fcntl(fd, F_GETFL) & O_APPEND) != 0);
    CHECK(pwrite(fd, "!", 1, 0) == 1); /* Linux applies O_APPEND even to pwrite. */
    CHECK(lseek(fd, 0, SEEK_CUR) == 2);
    CHECK(pread(fd, data, 7, 0) == 7 && memcmp(data, "aXYdef!", 7) == 0);
    CHECK(close(alias) == 0 && close(fd) == 0);
    CHECK(unlink(path) == 0);
    puts("PASS pread/pwrite, shared offsets/flags, fstatat and private file mmap");
    fflush(stdout);
    pid_t child = fork();
    CHECK(child >= 0);
    if (child == 0) { _exit(tls_value == 37 ? 42 : 1); }
    int status = 0;
    CHECK(waitpid(child, &status, 0) == child && WIFEXITED(status) && WEXITSTATUS(status) == 42);
    puts("PASS dynamic fork/wait and inherited TLS");
    fflush(stdout);
    char *args[] = {"preserved-argv-zero", "exec-child", NULL};
    execv("/bin/glibc-regression", args);
    perror("execv");
    return 1;
}
