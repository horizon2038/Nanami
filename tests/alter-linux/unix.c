/* Raw x86-64 AF_UNIX + SCM_RIGHTS integration; no libc or host network. */
typedef unsigned long Word;
typedef unsigned long long U64;
extern long linux_call(long, long, long, long, long, long, long);
void *memset(void *destination, int value, Word len) {
  volatile unsigned char *bytes = destination;
  for (Word i = 0; i < len; ++i) {
    bytes[i] = (unsigned char)value;
  }
  return destination;
}
#define sc(n, a, b, c) linux_call(n, (long)(a), (long)(b), (long)(c), 0, 0, 0)
#define sc4(n, a, b, c, d)                                                     \
  linux_call(n, (long)(a), (long)(b), (long)(c), (long)(d), 0, 0)
#define sc6(n, a, b, c, d, e, f)                                               \
  linux_call(n, (long)(a), (long)(b), (long)(c), (long)(d), (long)(e),         \
             (long)(f))
struct Address {
  unsigned short family;
  char path[108];
};
struct Iov {
  void *base;
  Word len;
};
struct Message {
  void *name;
  unsigned name_len;
  unsigned pad;
  struct Iov *iov;
  Word count;
  void *control;
  Word control_len;
  unsigned flags;
  unsigned tail;
};
struct Control {
  Word len;
  unsigned level;
  unsigned type;
  int fd[2];
};
static Word length(const char *s) {
  Word n = 0;
  while (s[n]) {
    ++n;
  }
  return n;
}
static void address(struct Address *a, const char *path) {
  a->family = 1;
  for (Word i = 0; i < 108; ++i) {
    a->path[i] = 0;
  }
  for (Word i = 0; path[i] && i < 107; ++i) {
    a->path[i] = path[i];
  }
}
static int pass_fd(int socket, int fd, const char *bytes, Word n) {
  struct Control control = {20, 1, 1, {fd, 0}};
  struct Iov iov = {(void *)bytes, n};
  struct Message msg = {0, 0, 0, &iov, 1, &control, 24, 0, 0};
  return (int)sc(46, socket, &msg, 0);
}
static int receive_fd(int socket, int *fd, char *bytes, Word n, int flags) {
  struct Control control = {0, 0, 0, {-1, -1}};
  struct Iov iov = {bytes, n};
  struct Message msg = {0, 0, 0, &iov, 1, &control, sizeof(control), 0, 0};
  long got = sc(47, socket, &msg, flags);
  if (got >= 0 && control.level == 1 && control.type == 1 &&
      control.len == 20) {
    *fd = control.fd[0];
  } else {
    *fd = -1;
  }
  return (int)got;
}
int test_main(void) {
  int sv[2];
  if (sc4(53, 1, 1 | 0x80800, 0, sv) != 0) {
    return 1;
  }
  if (sc(72, sv[0], 1, 0) != 1 || (sc(72, sv[0], 3, 0) & 0x803) != 0x802) {
    return 2;
  }
  if (sc(8, sv[0], 0, 0) != -29) {
    return 38;
  }
  unsigned credentials[3] = {0};
  unsigned size = 12;
  if (sc6(55, sv[0], 1, 17, credentials, &size, 0) != 0 || size != 12 ||
      credentials[0] != (unsigned)sc(39, 0, 0, 0)) {
    return 3;
  }
  long event = sc(290, 7, 0, 0);
  if (event < 0 || pass_fd(sv[0], (int)event, "x", 1) != 1 ||
      sc(3, event, 0, 0) != 0) {
    return 4;
  }
  int fd = -1;
  char byte = 0;
  if (receive_fd(sv[1], &fd, &byte, 1, 0x40000000) != 1 || fd < 0 ||
      byte != 'x' || sc(72, fd, 1, 0) != 1) {
    return 5;
  }
  U64 counter = 0;
  if (sc(0, fd, &counter, 8) != 8 || counter != 7 || sc(3, fd, 0, 0) != 0) {
    return 6;
  }
  int pipe[2];
  if (sc(22, pipe, 0, 0) != 0 || pass_fd(sv[0], pipe[0], "p", 1) != 1 ||
      sc(72, pipe[0], 4, 0x800) != 0 || sc(3, pipe[0], 0, 0) != 0) {
    return 39;
  }
  if (receive_fd(sv[1], &fd, &byte, 1, 0) != 1 || fd < 0 ||
      (sc(72, fd, 3, 0) & 0x800) == 0 || sc(0, fd, &byte, 1) != -11) {
    return 40;
  }
  sc(3, fd, 0, 0);
  sc(3, pipe[1], 0, 0);

  const char *file = "/tmp/unix-rights.txt";
  fd = (int)sc(2, file, 0x242, 0600);
  if (fd < 0 || sc(1, fd, "abcd", 4) != 4 || sc(8, fd, 1, 0) != 1 ||
      pass_fd(sv[0], fd, "r", 1) != 1 || sc(3, fd, 0, 0) != 0) {
    return 7;
  }
  if (receive_fd(sv[1], &fd, &byte, 1, 0) != 1 || fd < 0) {
    return 8;
  }
  char contents[3] = {0};
  if (sc(0, fd, contents, 2) != 2 || contents[0] != 'b' || contents[1] != 'c' ||
      sc(8, fd, 0, 1) != 3 || sc(3, fd, 0, 0) != 0) {
    return 9;
  }

  struct Address named;
  address(&named, "/tmp/wayland-test");
  long server = sc(41, 1, 1, 0);
  if (server < 0 || sc(49, server, &named, 3 + length(named.path)) != 0 ||
      sc(50, server, 2, 0) != 0) {
    return 10;
  }
  unsigned char stat[144] = {0};
  if (sc(4, named.path, stat, 0) != 0 ||
      (*(unsigned *)(stat + 24) & 0170000) != 0140000) {
    return 11;
  }
  long client = sc(41, 1, 1, 0);
  if (client < 0 || sc(42, client, &named, 3 + length(named.path)) != 0) {
    return 12;
  }
  long accepted = sc4(288, server, 0, 0, 0x80800);
  if (accepted < 0 || sc(1, client, "hello", 5) != 5) {
    return 13;
  }
  char hello[5] = {0};
  if (sc(0, accepted, hello, 5) != 5 || hello[0] != 'h' || hello[4] != 'o') {
    return 14;
  }
  struct Address renamed;
  address(&renamed, "/tmp/wayland-renamed");
  if (sc(82, named.path, renamed.path, 0) != 0) {
    return 15;
  }
  long next = sc(41, 1, 1, 0);
  if (next < 0 || sc(42, next, &renamed, 3 + length(renamed.path)) != 0) {
    return 16;
  }
  long second = sc(43, server, 0, 0);
  const char *alias = "/tmp/wayland-link";
  if (sc(86, renamed.path, alias, 0) != 0) {
    return 31;
  }
  if (second < 0 || sc(87, renamed.path, 0, 0) != 0) {
    return 17;
  }
  long new_server = sc(41, 1, 1, 0);
  if (new_server < 0 ||
      sc(49, new_server, &renamed, 3 + length(renamed.path)) != 0) {
    return 18;
  }
  struct Address linked;
  address(&linked, alias);
  long linked_client = sc(41, 1, 1, 0);
  if (linked_client < 0 ||
      sc(42, linked_client, &linked, 3 + length(linked.path)) != 0) {
    return 32;
  }
  long linked_peer = sc(43, server, 0, 0);
  if (linked_peer < 0 || sc(87, alias, 0, 0) != 0) {
    return 33;
  }
  /* The old inode is unlinked but still pinned; reusing this name is safe. */
  long linked_server = sc(41, 1, 1, 0);
  if (linked_server < 0 ||
      sc(49, linked_server, &linked, 3 + length(linked.path)) != 0) {
    return 34;
  }
  if (sc(48, client, 1, 0) != 0 || sc(0, accepted, &byte, 1) != 0 ||
      sc(1, accepted, "z", 1) != 1 || sc(0, client, &byte, 1) != 1 ||
      byte != 'z') {
    return 19;
  }
  sc(3, client, 0, 0);
  sc(3, accepted, 0, 0);
  sc(3, next, 0, 0);
  sc(3, second, 0, 0);
  sc(3, new_server, 0, 0);
  sc(3, server, 0, 0);
  sc(3, linked_peer, 0, 0);
  sc(3, linked_client, 0, 0);
  sc(3, linked_server, 0, 0);

  struct Address abstract = {1, {0, 'w', 'l', 0, 'x'}};
  server = sc(41, 1, 1, 0);
  client = sc(41, 1, 1, 0);
  if (server < 0 || client < 0 || sc(49, server, &abstract, 7) != 0 ||
      sc(50, server, 1, 0) != 0 || sc(42, client, &abstract, 7) != 0) {
    return 20;
  }
  accepted = sc(43, server, 0, 0);
  if (accepted < 0 || sc(1, client, "a", 1) != 1 ||
      sc(0, accepted, &byte, 1) != 1 || byte != 'a') {
    return 21;
  }
  sc(3, accepted, 0, 0);
  sc(3, client, 0, 0);
  sc(3, server, 0, 0);

  sc(3, sv[0], 0, 0);
  sc(3, sv[1], 0, 0);
  if (sc4(53, 1, 1, 0, sv) != 0) {
    return 22;
  }
  long child = sc(57, 0, 0, 0);
  if (child < 0) {
    return 23;
  }
  if (child == 0) {
    sc(3, sv[0], 0, 0);
    long delay[2] = {0, 2000000};
    sc(35, delay, 0, 0);
    long result = sc(1, sv[1], "wake", 4);
    long event = sc(290, 9, 0, 0);
    if (event < 0 || pass_fd(sv[1], (int)event, "f", 1) != 1) {
      sc(231, 35, 0, 0);
    }
    sc(3, event, 0, 0);
    sc(3, sv[1], 0, 0);
    sc(231, result == 4 ? 0 : 24, 0, 0);
    return 24;
  }
  sc(3, sv[1], 0, 0);
  long ep = sc(291, 0, 0, 0);
  unsigned char ev[12] = {1, 0, 0, 0, 0x78, 0x56, 0x34, 0x12};
  if (ep < 0 || sc4(233, ep, 1, sv[0], ev) != 0 ||
      sc4(232, ep, ev, 1, 1000) != 1) {
    return 25;
  }
  if (sc(0, sv[0], hello, 4) != 4 || hello[0] != 'w') {
    return 26;
  }
  if (receive_fd(sv[0], &fd, &byte, 1, 0) != 1 || byte != 'f' || fd < 0 ||
      sc(0, fd, &counter, 8) != 8 || counter != 9) {
    return 36;
  }
  sc(3, fd, 0, 0);
  if (sc(0, sv[0], hello, 1) != 0) {
    return 37;
  }
  int status = -1;
  if (sc4(61, child, &status, 0, 0) != child || status != 0) {
    return 27;
  }
  sc(3, sv[0], 0, 0);
  sc(3, ep, 0, 0);

  for (int kind = 2; kind <= 5; kind += 3) {
    if (sc4(53, 1, kind | 0x800, 0, sv) != 0 ||
        sc(1, sv[0], "record", 6) != 6) {
      return 28;
    }
    char short_data[2];
    if (sc6(45, sv[1], short_data, 2, 0x20, 0, 0) != 6 ||
        short_data[0] != 'r' || short_data[1] != 'e') {
      return 29;
    }
    sc(3, sv[0], 0, 0);
    sc(3, sv[1], 0, 0);
  }
  /* Repeated close must collect in-flight UNIX descriptor cycles. */
  for (int i = 0; i < 160; ++i) {
    if (sc4(53, 1, 1, 0, sv) != 0 || pass_fd(sv[0], sv[1], "c", 1) != 1) {
      return 30;
    }
    sc(3, sv[0], 0, 0);
    sc(3, sv[1], 0, 0);
  }
  const char ok[] = "unix-socket-ok\n";
  sc(1, 1, ok, sizeof(ok) - 1);
  return 0;
}
