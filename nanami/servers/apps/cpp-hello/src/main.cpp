#include <nanami/nanami.hpp>

extern "C" int main(void) {
  nanami::write_string("[user-app/cpp] hello from C++ user process\n");
  a9n::Word echoed = 0;
  if (nanami::ping(0xfeedbeef, &echoed) == a9n::NanamiStatus::Ok &&
      echoed == 0xfeedbeef) {
    nanami::write_string("[user-app/cpp] ping-pong ok\n");
  } else {
    nanami::write_string("[user-app/cpp] ping-pong failed\n");
  }
  (void)nanami::request_pages(1);
  if (nanami::request_exit() != a9n::NanamiStatus::Ok) {
    nanami::write_string("[user-app/cpp] exit failed\n");
    for (;;) {
      asm volatile("pause");
    }
  }
  // request_exit may return before scheduler actually tears down this task.
  // Never return to the process entry trampoline.
  for (;;) {
    asm volatile("pause");
  }
}
