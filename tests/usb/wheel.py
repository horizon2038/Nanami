"""End-to-end wheel checks against Shell scrollback (USB mouse via QMP)."""
import time


def exercise(qmp, type_text, logs):
    # More output than one Shell page; the same up/down count must restore it.
    type_text('help\n' * 5 + 'echo wheel-bottom\n')
    time.sleep(2)
    qmp('screendump', {'filename': str(logs / 'wheel-size.ppm')})
    _, dimensions, _, _ = (logs / 'wheel-size.ppm').read_bytes().split(b'\n', 3)
    width, height = map(int, dimensions.split())
    dx, dy = 300 - width * 3 // 4, 200 - height // 2

    def move(x, y):
        qmp('input-send-event', {'events': [
            {'type': 'rel', 'data': {'axis': 'x', 'value': x}},
            {'type': 'rel', 'data': {'axis': 'y', 'value': y}},
        ]})
        time.sleep(2)

    def screen(name):
        path = logs / (name + '.ppm')
        qmp('screendump', {'filename': str(path)})
        _, _, _, pixels = path.read_bytes().split(b'\n', 3)
        # Text only, excluding the desktop clock/title and blinking prompt row.
        return b''.join(pixels[(y * width + 100) * 3:(y * width + 750) * 3]
                        for y in range(130, 430))

    move(dx, dy)  # Honoka delivers wheel events to the content under the pointer.
    bottom = screen('wheel-before')
    for direction in ['wheel-up', 'wheel-down']:
        for _ in range(4):
            for down in [True, False]:
                qmp('input-send-event', {'events': [{'type': 'btn', 'data': {
                    'down': down, 'button': direction}}]})
                time.sleep(0.15)
        time.sleep(3)
        current = screen(direction)
        if direction == 'wheel-up':
            assert current != bottom, 'USB wheel-up did not scroll Shell'
        else:
            assert current == bottom, 'USB wheel-down did not restore Shell scrollback'
    move(-dx, -dy)
    print('PASS: USB wheel scrolls Shell up and back down pixel-exactly', flush=True)
