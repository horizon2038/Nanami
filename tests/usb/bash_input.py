"""Exercise untraced bash input using writes to the disposable USB snapshot."""
import re
import struct
import time


def run_doom_first(qmp, type_text, wait_for, logs, save=False, fb_size=None):
    options = f'--fb-size {fb_size} ' if fb_size else ''
    type_text(f'alter -g {options}/alter/linux/bin/doomgeneric-fbdev -iwad /bin/doom1.wad\n')
    wait_for(lambda text: 'managed rootfs process image=doomgeneric-fbdev ' in text)
    time.sleep(10)
    qmp('screendump', {'filename': str(logs / 'doom-running.png'), 'format': 'png'})
    if save:
        from doom_save import exercise
        exercise(qmp, logs)
    # Doom's normal quit confirmation, rather than destroying the process from
    # the test harness. The subsequent bash launch must succeed via Shell.
    qmp('send-key', {'keys': [{'type': 'qcode', 'data': 'f10'}], 'hold-time': 100})
    time.sleep(2)
    qmp('send-key', {'keys': [{'type': 'qcode', 'data': 'y'}], 'hold-time': 100})
    time.sleep(8)
    qmp('screendump', {'filename': str(logs / 'after-doom.png'), 'format': 'png'})
    # Destroying the focused Doom window leaves no keyboard focus. Click Shell
    # before typing, then restore the cursor for the later motion assertions.
    width, height = struct.unpack('>II', (logs / 'after-doom.png').read_bytes()[16:24])
    focus_shell(qmp, width, height)
    print('Requested normal Doom quit; checking subsequent untraced bash input', flush=True)


def focus_shell(qmp, width, height):
    """Click Shell after a graphics client exits; restore the known cursor position."""
    dx, dy = 300 - width * 3 // 4, 200 - height // 2
    for x, y in [(dx, dy), (-dx, -dy)]:
        qmp('input-send-event', {'events': [
            {'type': 'rel', 'data': {'axis': 'x', 'value': x}},
            {'type': 'rel', 'data': {'axis': 'y', 'value': y}},
        ]})
        time.sleep(2)
        if (x, y) == (dx, dy):
            for down in (True, False):
                qmp('input-send-event', {'events': [{'type': 'btn', 'data': {
                    'down': down, 'button': 'left'}}]})
                time.sleep(0.3)


def exercise(qmp, type_text, wait_for, serial, written_bytes, rounds):
    type_text('alter /alter/linux/bin/bash\n')
    wait_for(lambda text: re.search(r'managed rootfs process image=bash pid=(\d+)', text))
    # Untraced bash intentionally emits no read/exit serial diagnostics.
    time.sleep(5)
    for index in range(rounds):
        before = written_bytes()
        # Exercise readline insertion/erase and a child process, without -t's
        # additional terminal IPC and serial writes changing the schedule.
        command = f'/bin/busybox mkdir /input{index}xx\b\b\n'
        for char in command:
            key = {' ': 'spc', '/': 'slash', '\n': 'ret', '\b': 'backspace'}.get(char, char)
            qmp('send-key', {'keys': [{'type': 'qcode', 'data': key}], 'hold-time': 40})
            time.sleep(0.1)
        deadline = time.monotonic() + 20
        while written_bytes() <= before and time.monotonic() < deadline:
            time.sleep(0.1)
        assert written_bytes() > before, f'Untraced bash input stalled at round {index + 1}'
        print(f'PASS: untraced bash input/write round {index + 1}/{rounds}', flush=True)
    assert 'read stdin pid=' not in serial.read_text(errors='replace'), 'Bash unexpectedly traced'
    type_text('exit\n')
    time.sleep(3)
    before = written_bytes()
    type_text('mkdir afterbash\n')
    deadline = time.monotonic() + 20
    while written_bytes() <= before and time.monotonic() < deadline:
        time.sleep(0.1)
    assert written_bytes() > before, 'No native Shell write after untraced bash exit'
    print('PASS: keyboard/storage still respond after untraced bash exit', flush=True)
