"""Check per-launch fbdev dimensions on the disposable QEMU image."""
import re
import struct
import time
from bash_input import focus_shell


def exercise(qmp, type_text, wait_for, serial, logs, case='all'):
    cases = [
        ('--fb-size 640x400 ', 640, 400),
        ('--fb-size 641x401 ', 641, 401),
        ('', 800, 600),
        ('--fb-size 4096x2160 ', 0, 0),
    ]
    if case == 'default':
        cases = [cases[2]]
    elif case == 'clamped':
        cases = [cases[3]]
    elif case == 'stress':
        cases = [cases[0]]
    for options, width, height in cases:
        start = len(serial.read_text(errors='replace'))
        mode = ' child' if case == 'clamped' else ' stress' if case == 'stress' else ''
        type_text(f'alter -t -g {options}/alter/linux/bin/fb-size-test {width} {height}{mode}\n')
        pattern = r'managed rootfs process image=fb-size-test pid=(\d+)'
        wait_for(lambda s: re.search(pattern, s[start:]))
        pid = re.search(pattern, serial.read_text(errors='replace')[start:])[1]
        exit_pattern = r'exit pid=' + pid + r' syscall=\d+ status=(\d+)\b'
        wait_for(lambda s: re.search(exit_pattern, s[start:]), timeout=300 if case == 'stress' else 120)
        status = re.search(exit_pattern, serial.read_text(errors='replace')[start:])[1]
        assert status == '0', f'fbdev {options or "default"} failed: {status}'
        checks = 'stat/ioctl/read/write/mmap/remap' + ('' if mode else '/fork/exec')
        if case == 'stress':
            checks += ' (128 mappings)'
        print(f'PASS: fbdev {options or "default"}: {checks}', flush=True)
        time.sleep(2)
        shot = logs / 'after-framebuffer.png'
        qmp('screendump', {'filename': str(shot), 'format': 'png'})
        width_pixels, height_pixels = struct.unpack('>II', shot.read_bytes()[16:24])
        focus_shell(qmp, width_pixels, height_pixels)
