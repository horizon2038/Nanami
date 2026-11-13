"""Check completed pci-info output to the right of Shell's initial 89 columns."""
import time


def exercise(qmp, serial, logs):
    def text(value):
        for char in value:
            key = {'-': 'minus', '\n': 'ret', ' ': 'spc', '/': 'slash'}.get(char, char)
            qmp('send-key', {'keys': [{'type': 'qcode', 'data': key}], 'hold-time': 40})
            time.sleep(0.15)
        time.sleep(3)  # includes process exit and Shell's scrollback repaint

    def motion(dx, dy):
        while dx or dy:
            sx, sy = max(-100, min(100, dx)), max(-100, min(100, dy))
            qmp('input-send-event', {'events': [
                {'type': 'rel', 'data': {'axis': 'x', 'value': sx}},
                {'type': 'rel', 'data': {'axis': 'y', 'value': sy}},
            ]})
            dx -= sx
            dy -= sy
            time.sleep(0.06)
        time.sleep(1)

    def resize(x, y, dx, dy):
        motion(-4000, -4000)
        motion(x, y)
        qmp('input-send-event', {'events': [{'type': 'btn', 'data': {'down': True, 'button': 'left'}}]})
        time.sleep(0.3)
        motion(dx, dy)
        qmp('input-send-event', {'events': [{'type': 'btn', 'data': {'down': False, 'button': 'left'}}]})
        time.sleep(2)

    def capture(name):
        path = logs / (name + '.ppm')
        qmp('screendump', {'filename': str(path)})
        magic, dimensions, maximum, pixels = path.read_bytes().split(b'\n', 3)
        assert magic == b'P6' and maximum == b'255'
        width, height = map(int, dimensions.split())
        assert len(pixels) == width * height * 3
        return width, height, pixels

    def region(picture, x, y, width, height):
        stride, rows, pixels = picture
        assert x + width <= stride and y + height <= rows
        return b''.join(pixels[((y + row) * stride + x) * 3:((y + row) * stride + x + width) * 3]
                        for row in range(height))

    time.sleep(3)
    resize(802, 500, 192, 48)  # 712 -> 904 content pixels: 89 -> 113 columns
    text('clear\n')
    empty = capture('wide-empty')
    text('pci-info\n')
    populated = capture('wide-pci-info')
    # Content begins at (94,108). Exclude prompt/cursor and window decoration.
    box = (94 + 96 * 8, 108 + 2 * 12, 14 * 8, 24 * 12)
    before = region(empty, *box)
    after = region(populated, *box)
    changed = sum(a != b for a, b in zip(before, after))
    assert changed > 100, f'Completed pci-info remained limited to initial width: changed bytes={changed}'

    # Exercise a built-in, then shrink and run another completed external app.
    text('cat /nanami/system-list\n')
    capture('wide-cat')
    resize(994, 548, -192, -48)
    text('clear\n')
    text('pci-info\n')
    capture('restored-pci-info')
    log = serial.read_text(errors='replace')
    assert '[shell] panic' not in log and 'resize failed:' not in log
    print(f'PASS Shell 89 -> 113 -> 89 columns: completed pci-info right-side ink={changed}, built-in cat and shrink/grow', flush=True)
