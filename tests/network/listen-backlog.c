// Test-only Darwin interposer: libslirp's host-forward listener uses backlog 1.
// This changes only the launched QEMU process, not the OS or installed libslirp.
#include <sys/socket.h>
#include <stdio.h>

static int test_listen(int fd, int backlog) {
    if (backlog == 1) {
        fprintf(stderr, "[test] libslirp listener fd=%d: backlog 1 -> SOMAXCONN (%d)\n", fd, SOMAXCONN);
        backlog = SOMAXCONN;
    }
    return listen(fd, backlog);
}

__attribute__((used, section("__DATA,__interpose")))
static struct { const void *replacement; const void *original; } interpose = {
    (const void *)test_listen, (const void *)listen
};
