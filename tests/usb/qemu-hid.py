#!/usr/bin/env python3
"""Boot a snapshot with PS/2 disabled, then exercise USB HID and hotplug.

No physical USB passthrough or host disk writes. The only network is QEMU's
restricted user-mode NIC; there is no forwarding or LAN injection.
"""
import argparse
import json
import pathlib
import shutil
import socket
import subprocess
import tempfile
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--image', type=pathlib.Path, required=True)
parser.add_argument('--smp', type=int, default=4)
parser.add_argument('--coexist-ps2', action='store_true')
parser.add_argument('--stress', type=int, default=0, help='Number of modifier press/release pairs before typing')
parser.add_argument('--high-mmio', action='store_true', help='Check high-BAR refusal and continued desktop boot')
args = parser.parse_args()
repo = pathlib.Path(__file__).resolve().parents[2]
logs = pathlib.Path(tempfile.mkdtemp(prefix='nanami-usb-test-'))
firmware = repo / 'spencer/a9nloader-rs/tools'
shutil.copyfile(firmware / 'OVMF_VARS.fd', logs / 'OVMF_VARS.fd')
serial = logs / 'serial.log'
qmp_path = logs / 'qmp.sock'
command = [
    'qemu-system-x86_64', '-machine', f'hpet=on,i8042={"on" if args.coexist_ps2 or args.high_mmio else "off"}',
    '-cpu', 'max', '-smp', str(args.smp), '-m', '4G', '-display', 'none',
    *([] if args.high_mmio else ['-fw_cfg', 'name=opt/ovmf/X-PciMmio64Mb,string=0']),
    '-serial', f'file:{serial}', '-qmp', f'unix:{qmp_path},server=on,wait=off',
    '-netdev', 'user,id=net0,restrict=on',
    '-device', 'virtio-net,netdev=net0,addr=5,disable-legacy=off,disable-modern=on',
    '-device', 'qemu-xhci,id=xhci,addr=6,msi=off,msix=off',
    '-device', 'usb-kbd,id=usb-kbd,bus=xhci.0,port=1',
    '-device', 'usb-mouse,id=usb-mouse,bus=xhci.0,port=2',
    '-drive', f'if=pflash,format=raw,readonly=on,file={firmware / "OVMF_CODE.fd"}',
    '-drive', f'if=pflash,format=raw,file={logs / "OVMF_VARS.fd"}',
    '-device', 'ich9-ahci,id=ahci,addr=3',
    '-drive', f'if=none,id=disk,format=raw,snapshot=on,file={args.image.resolve()}',
    '-device', 'ide-hd,drive=disk,bus=ahci.0,bootindex=1', '--no-reboot', '--no-shutdown',
]

print(f'Logs: {logs}', flush=True)
with (logs / 'qemu.log').open('w') as diagnostics:
    process = subprocess.Popen(command, stdout=diagnostics, stderr=diagnostics)
    stream = None
    connection = None
    try:
        for _ in range(100):
            if qmp_path.exists():
                break
            time.sleep(0.1)
        connection = socket.socket(socket.AF_UNIX)
        connection.settimeout(10)
        connection.connect(str(qmp_path))
        stream = connection.makefile('rwb', buffering=0)
        json.loads(stream.readline())

        def qmp(name, arguments=None):
            message = {'execute': name}
            if arguments is not None:
                message['arguments'] = arguments
            stream.write(json.dumps(message).encode() + b'\n')
            while True:
                answer = json.loads(stream.readline())
                if 'error' in answer:
                    raise RuntimeError(answer)
                if 'return' in answer:
                    return answer['return']

        def wait_for(predicate, timeout=120):
            deadline = time.monotonic() + timeout
            while time.monotonic() < deadline:
                if process.poll() is not None:
                    raise RuntimeError('QEMU exited')
                if serial.exists() and predicate(serial.read_text(errors='replace')):
                    return
                time.sleep(0.2)
            qmp('screendump', {'filename': str(logs / 'timeout.ppm')})
            raise RuntimeError('Guest timed out; inspect serial.log')

        qmp('qmp_capabilities')
        if args.high_mmio:
            wait_for(lambda s: 'exceeds current low-MMIO support' in s
                     and 'font init end' in s and '[              honoka] online' in s)
            assert 'usb-server] xHCI ports=' not in serial.read_text()
            print('PASS: high BAR refused; storage, Shell and desktop still booted', flush=True)
            raise SystemExit(0)
        wait_for(lambda s: 'usb-server] online controllers=' in s
                 and s.count('boot-hid=1') >= 2 and 'font init end' in s and '[              honoka] online' in s)
        print('USB keyboard/mouse enumerated; desktop ready', flush=True)
        time.sleep(5)
        for _ in range(args.stress):
            for down in (True, False):
                qmp('input-send-event', {'events': [{'type': 'key', 'data': {
                    'down': down, 'key': {'type': 'qcode', 'data': 'shift'}}}]})
                time.sleep(0.05)
        # QEMU directs events to the USB keyboard/mouse. By default the VM has
        # no i8042 controller, so a PS/2 driver cannot make this test pass.
        def type_text(text):
            for char in text:
                key = {'-': 'minus', '\n': 'ret'}.get(char, char)
                qmp('send-key', {'keys': [{'type': 'qcode', 'data': key}], 'hold-time': 50})
                time.sleep(0.8)

        type_text('http-server\n')
        qmp('screendump', {'filename': str(logs / 'after-keyboard.ppm')})
        wait_for(lambda s: 'tcp listen port=80' in s)
        print('USB keyboard launched HTTP', flush=True)
        time.sleep(2)
        qmp('screendump', {'filename': str(logs / 'before-mouse.ppm')})
        qmp('input-send-event', {'events': [
            {'type': 'rel', 'data': {'axis': 'x', 'value': 120}},
            {'type': 'rel', 'data': {'axis': 'y', 'value': 60}},
        ]})
        time.sleep(1)
        qmp('screendump', {'filename': str(logs / 'after-mouse.ppm')})
        def cursor_region(path, x, y):
            magic, dimensions, depth, pixels = path.read_bytes().split(b'\n', 3)
            assert magic == b'P6' and depth == b'255'
            width, height = map(int, dimensions.split())
            return b''.join(pixels[((y + row) * width + x) * 3:((y + row) * width + x + 24) * 3]
                            for row in range(24))

        _, dimensions, _, _ = (logs / 'before-mouse.ppm').read_bytes().split(b'\n', 3)
        width, height = map(int, dimensions.split())
        for x, y in [(width * 3 // 4, height // 2), (width * 3 // 4 + 120, height // 2 + 60)]:
            if cursor_region(logs / 'before-mouse.ppm', x, y) == cursor_region(logs / 'after-mouse.ppm', x, y):
                raise RuntimeError(f'USB cursor did not leave/enter expected region {x},{y}')
        # Stop the foreground HTTP process, then hold Shift across USB removal.
        # The subsequent lowercase command must not inherit a stuck modifier.
        qmp('send-key', {'keys': [{'type': 'qcode', 'data': 'f12'}], 'hold-time': 100})
        time.sleep(2)
        qmp('input-send-event', {'events': [{'type': 'key', 'data': {
            'down': True, 'key': {'type': 'qcode', 'data': 'shift'}}}]})
        time.sleep(0.5)
        qmp('device_del', {'id': 'usb-kbd'})
        qmp('device_del', {'id': 'usb-mouse'})
        wait_for(lambda s: s.count('usb-server] detached port=') >= 2)
        if args.coexist_ps2:
            qmp('input-send-event', {'events': [{'type': 'key', 'data': {
                'down': False, 'key': {'type': 'qcode', 'data': 'shift'}}}]})
            # With both USB devices gone, only the PS/2 keyboard can deliver it.
            type_text('http-server\n')
            wait_for(lambda s: s.count('http-server] bootstrap') >= 2)
            qmp('send-key', {'keys': [{'type': 'qcode', 'data': 'f12'}], 'hold-time': 100})
            time.sleep(2)
        qmp('device_add', {'driver': 'usb-kbd', 'id': 'usb-kbd-2', 'bus': 'xhci.0', 'port': '1'})
        qmp('device_add', {'driver': 'usb-mouse', 'id': 'usb-mouse-2', 'bus': 'xhci.0', 'port': '2'})
        wait_for(lambda s: s.count('boot-hid=1') >= 4)
        time.sleep(1)
        type_text('http-server\n')
        wait_for(lambda s: s.count('http-server] bootstrap') >= (3 if args.coexist_ps2 else 2))
        qmp('send-key', {'keys': [{'type': 'qcode', 'data': 'f12'}], 'hold-time': 100})
        time.sleep(2)
        # The default Shell window starts at (90,80), with close at (793,93).
        # Exercise relative movement spanning multiple boot-mouse reports and
        # the replugged mouse's left button, not just its enumeration.
        qmp('screendump', {'filename': str(logs / 'before-click.ppm')})
        qmp('input-send-event', {'events': [
            {'type': 'rel', 'data': {'axis': 'x', 'value': 793 - (width * 3 // 4 + 120)}},
            {'type': 'rel', 'data': {'axis': 'y', 'value': 93 - (height // 2 + 60)}},
        ]})
        time.sleep(2)
        for down in (True, False):
            qmp('input-send-event', {'events': [{'type': 'btn', 'data': {'down': down, 'button': 'left'}}]})
            time.sleep(0.3)
        time.sleep(2)
        qmp('screendump', {'filename': str(logs / 'after-click.ppm')})
        if cursor_region(logs / 'before-click.ppm', 110, 85) == cursor_region(logs / 'after-click.ppm', 110, 85):
            raise RuntimeError('Replugged USB mouse did not close Shell')
        print('PASS: keyboard launched HTTP before/after hotplug; cursor moved; replugged mouse clicked close; detach released Shift'
              + ('; PS/2 keyboard also worked with USB unplugged' if args.coexist_ps2 else ' (PS/2 disabled)'), flush=True)
    finally:
        if stream is not None:
            try:
                stream.write(b'{"execute":"quit"}\n')
            except OSError:
                pass
            stream.close()
        if connection is not None:
            connection.close()
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.terminate()
            process.wait(timeout=5)
