"""BusyBox vi, native command editing and resize against a private VM snapshot."""
import re
import time


def exercise(qmp, wait_for, serial, logs):
    def key(code, shifted=False):
        codes = (['shift'] if shifted else []) + [code]
        qmp('send-key', {'keys': [{'type': 'qcode', 'data': k} for k in codes], 'hold-time': 40})
        time.sleep(0.2)

    def text(value):
        for char in value:
            code = {' ': 'spc', '/': 'slash', '-': 'minus', '\n': 'ret', '.': 'dot', ':': 'semicolon'}.get(char, char)
            key(code, char == ':')

    def exited_since(start):
        pattern = r'exit pid=\d+ syscall=\d+ status=(\d+)\b'
        wait_for(lambda s: re.search(pattern, s[start:]))
        result = re.search(pattern, serial.read_text(errors='replace')[start:])[1]
        assert result == '0', f'guest command exited with {result}'
        time.sleep(1)

    start = len(serial.read_text(errors='replace'))
    text('alter -t /alter/linux/bin/busybox pd')
    key('left')
    text('w\n')
    exited_since(start)
    start = len(serial.read_text(errors='replace'))
    key('up')
    key('ret')
    exited_since(start)
    print('PASS: native Shell insertion at cursor and command history', flush=True)

    # Resize Shell by its lower-right grip: native content is 712x396,
    # outer frame starts at (90,78), decoration is 8x34.
    def move_to(x, y):
        for dx, dy in [(-4000, -4000), (x, y)]:
            # Do not leave dozens of boot-mouse reports pending in QEMU when
            # the button is pressed (especially under single-core TCG).
            while dx or dy:
                sx, sy = max(-100, min(100, dx)), max(-100, min(100, dy))
                qmp('input-send-event', {'events': [
                    {'type': 'rel', 'data': {'axis': 'x', 'value': sx}},
                    {'type': 'rel', 'data': {'axis': 'y', 'value': sy}},
                ]})
                dx -= sx
                dy -= sy
                time.sleep(0.06)
            time.sleep(2)

    def resize(x, y, dx, dy):
        move_to(x, y)
        qmp('input-send-event', {'events': [{'type': 'btn', 'data': {'down': True, 'button': 'left'}}]})
        time.sleep(0.3)
        while dx or dy:
            sx, sy = max(-100, min(100, dx)), max(-100, min(100, dy))
            qmp('input-send-event', {'events': [
                {'type': 'rel', 'data': {'axis': 'x', 'value': sx}},
                {'type': 'rel', 'data': {'axis': 'y', 'value': sy}},
            ]})
            dx -= sx
            dy -= sy
            time.sleep(0.1)
        time.sleep(2)
        qmp('input-send-event', {'events': [{'type': 'btn', 'data': {'down': False, 'button': 'left'}}]})
        time.sleep(2)

    resize(802, 500, 96, 48)
    qmp('screendump', {'filename': str(logs / 'shell-resized.png'), 'format': 'png'})
    text('alter /alter/linux/bin/busybox vi /tmp/vi-test\n')
    wait_for(lambda s: s.count('managed rootfs process image=busybox ') >= 3)
    time.sleep(3)
    text('ifirst line\nsecond line')
    key('up')
    key('home')
    text('new ')
    key('esc')
    time.sleep(2)
    qmp('screendump', {'filename': str(logs / 'busybox-vi.png'), 'format': 'png'})
    text(':wq\n')
    time.sleep(4)

    # Reopen an existing file, shorten it, write a named copy, then save/exit.
    # Both writes invoke ftruncate: stale trailing text must not survive.
    text('alter /alter/linux/bin/busybox vi /tmp/vi-test\n')
    wait_for(lambda s: s.count('managed rootfs process image=busybox ') >= 4)
    time.sleep(3)
    text('ggdd:w /tmp/vi-copy\n')
    time.sleep(1)
    text(':w /tmp/vi-test\n')
    time.sleep(1)
    text(':x\n')
    time.sleep(4)

    # Check contents through a Linux utility, not a visual-only assertion.
    start = len(serial.read_text(errors='replace'))
    text('alter -t /alter/linux/bin/terminal-test\n')
    exited_since(start)
    qmp('screendump', {'filename': str(logs / 'terminal-size.png'), 'format': 'png'})
    resize(898, 548, -736, -412)  # minimum 72x32 content
    qmp('screendump', {'filename': str(logs / 'shell-minimum.png'), 'format': 'png'})
    resize(162, 136, 640, 364)
    start = len(serial.read_text(errors='replace'))
    text('alter -t /alter/linux/bin/busybox pwd\n')
    exited_since(start)
    log = serial.read_text(errors='replace')
    assert '[shell] panic' not in log and '[shell] resize failed' not in log
    assert 'unsupported syscall 77' not in log
    print('PASS: BusyBox vi file contents, escaped USB arrows, TIOCGWINSZ, termios and shrink/grow liveness', flush=True)
