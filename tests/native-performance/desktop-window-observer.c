/* Exercise the actual test observer's title rules without an X server. */
#include <assert.h>
#define main observer_main
#include "../alter-linux/desktop-window-observer.c"
#undef main

int main(void) {
    assert(title_matches("Terminal", "terminal"));
    assert(!title_matches("Tiny Core on Alter/Linux", "terminal"));
    assert(!title_matches("Terminal error", "terminal"));
    assert(title_matches("Xfe", "files"));
    assert(title_matches("File Explorer - /root", "files"));
    assert(!title_matches("Untitled", "files"));
    assert(title_matches("Untitled", "editor"));
    assert(title_matches("example - Editor", "editor"));
    assert(!title_matches("Tiny Core on Alter/Linux", "editor"));
    assert(!title_matches("wbar", "editor"));
    assert(!contains("", "editor"));
    assert(!contains("ed", "editor"));
    observer_root = 1;
    union app_event event = {0};
    event.client.type = 33;
    event.client.format = 32;
    event.client.window = observer_root;
    event.client.message_type = 12;
    event.client.data[0] = 13;
    event.client.data[1] = 14;
    event.client.data[2] = 15;
    assert(ping_reply(&event, 12, 13, 14, 15));
    assert(!ping_reply(&event, 12, 13, 14, 16));
    assert(!ping_reply(&event, 12, 13, 16, 15));
    assert(!ping_reply(&event, 12, 16, 14, 15));
    assert(!ping_reply(&event, 16, 13, 14, 15));
    event.client.window = 15;
    assert(!ping_reply(&event, 12, 13, 14, 15));
    event.client.window = observer_root;
    event.client.format = 8;
    assert(!ping_reply(&event, 12, 13, 14, 15));
    event.client.format = 32;
    event.client.type = 28;
    assert(!ping_reply(&event, 12, 13, 14, 15));
    return 0;
}
