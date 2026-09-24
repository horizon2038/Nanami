"""Exercise each native GUI client's surface replacement, including tiny sizes."""
import pathlib
import struct
import time


def exercise(qmp, logs, serial):
    def text(value):
        for char in value:
            key = {'-': 'minus', '\n': 'ret'}.get(char, char)
            qmp('send-key', {'keys': [{'type': 'qcode', 'data': key}], 'hold-time': 40})
            time.sleep(0.2)

    def move(x, y):
        for dx, dy in [(-4000, -4000), (x, y)]:
            qmp('input-send-event', {'events': [
                {'type': 'rel', 'data': {'axis': 'x', 'value': dx}},
                {'type': 'rel', 'data': {'axis': 'y', 'value': dy}}]})
            time.sleep(0.6)

    def button(down):
        qmp('input-send-event', {'events': [{'type': 'btn', 'data': {'down': down, 'button': 'left'}}]})
        time.sleep(0.2)

    def resize(x, y, width, height, new_width, new_height):
        move(x + width, y + height + 26)
        button(True)
        qmp('input-send-event', {'events': [
            {'type': 'rel', 'data': {'axis': 'x', 'value': new_width - width}},
            {'type': 'rel', 'data': {'axis': 'y', 'value': new_height - height}}]})
        time.sleep(0.2)
        button(False)
        time.sleep(2)

    def capture(name):
        path = logs / (name + '.ppm')
        qmp('screendump', {'filename': str(path)})
        _, dimensions, _, pixels = path.read_bytes().split(b'\n', 3)
        width, height = map(int, dimensions.split())
        return width, height, pixels

    def title(picture, x, y):
        width, _, pixels = picture
        return b''.join(pixels[((y + row) * width + x + 10) * 3:((y + row) * width + x + 65) * 3]
                        for row in range(8, 24))

    root = pathlib.Path(__file__).resolve().parents[2]
    bmp = (root / 'nanami/servers/apps/image-viewer/assets/image.bmp').read_bytes()
    iw, ih = struct.unpack_from('<ii', bmp, 18)
    clients = [
        ('eg-test', 640, 130, 552, 326),
        ('honoka-client', 760, 120, 512, 306),
        ('performance-monitor', 180, 120, 520, 330),
        ('image-viewer', 260, 160, max(iw + 24, 420), abs(ih) + 78),
        ('saran', 90, 78, 712, 396),
    ]
    for name, x, y, width, height in clients:
        move(300, 90)  # Focus Shell after the preceding client exits.
        button(True)
        button(False)
        text(name + '\n')
        time.sleep(6)
        capture(name + '-initial')
        resize(x, y, width, height, 72, 32)
        capture(name + '-minimum')
        resize(x, y, 72, 32, width + 48, height + 24)
        before = capture(name + '-resized')
        move(x + width + 48 + 8 - 17, y + 15)
        button(True)
        button(False)
        time.sleep(3)
        after = capture(name + '-closed')
        assert title(before, x, y) != title(after, x, y), f'{name}: resized close button did not close the client'
        log = serial.read_text(errors='replace')
        assert 'resize failed:' not in log and 'panic' not in log
        print(f'PASS: {name} shrink/grow, redraw and close at new geometry', flush=True)
