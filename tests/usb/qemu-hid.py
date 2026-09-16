#!/usr/bin/env python3
"""Boot a snapshot with PS/2 disabled, then exercise USB HID and hotplug.

No physical USB passthrough or host disk writes. The only network is QEMU's
restricted user-mode NIC; there is no forwarding or LAN injection.
"""
import argparse
import json
import pathlib
import re
import shutil
import socket
import subprocess
import sys
import tempfile
import time
from late_root import boot_without_root
from bash_input import exercise as exercise_bash_input, run_doom_first
from wheel import exercise as exercise_wheel

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--image', type=pathlib.Path, required=True)
parser.add_argument('--smp', type=int, default=4)
parser.add_argument('--memory', default='4G')
parser.add_argument('--coexist-ps2', action='store_true')
parser.add_argument('--hpet', choices=('on', 'off'), default='on')
parser.add_argument('--bash-smoke', action='store_true', help='Exercise interactive bash forks instead of HTTP/hotplug (requires bash and busybox in rootfs)')
parser.add_argument('--bash-input-stress', type=int, default=0, help='Exercise this many untraced bash input/edit/write cycles (requires --bash-smoke --usb-storage)')
parser.add_argument('--doom-first', action='store_true', help='Launch graphics Doom and request normal quit before --bash-input-stress')
parser.add_argument('--wheel-smoke', action='store_true', help='Verify USB wheel-up/down scroll Shell and restore its text pixels')
parser.add_argument('--no-network', action='store_true', help='Omit the virtual NIC (requires --bash-smoke)')
parser.add_argument('--stress', type=int, default=0, help='Number of modifier press/release pairs before typing')
parser.add_argument('--mouse-stress', type=int, default=0, help='Inject sustained relative motion before the normal liveness checks')
parser.add_argument('--drag-stress', action='store_true', help='Run the mouse stress while dragging Shell by its title bar')
parser.add_argument('--high-mmio', action='store_true', help='Keep the firmware default high PCI BAR assignment')
parser.add_argument('--usb-storage', action='store_true', help='Boot/rootfs on USB BOT, with AHCI present but no SATA media')
parser.add_argument('--virtio-storage', action='store_true', help='Boot/rootfs on legacy virtio-blk instead of AHCI')
parser.add_argument('--usb2', action='store_true', help='Make the storage connector USB2-only to test high-speed BOT')
parser.add_argument('--duplicate-root', action='store_true', help='Attach another Nanami USB root and require fail-closed selection')
parser.add_argument('--unplug-root', action='store_true', help='Unplug/replug the USB root and check that I/O never binds the replacement')
parser.add_argument('--empty-sata', action='store_true', help='Probe a non-root SATA disk while booting from USB')
parser.add_argument('--late-root', action='store_true', help='Boot firmware/initramfs from a private rootless SATA clone, then attach USB root after usb-server is online')
args = parser.parse_args()
if args.wheel_smoke and args.drag_stress:
    parser.error('--wheel-smoke requires the initial Shell position (no --drag-stress)')
if args.doom_first and not args.bash_input_stress:
    parser.error('--doom-first requires --bash-input-stress')
if args.bash_input_stress < 0 or (args.bash_input_stress and not (args.bash_smoke and args.usb_storage)):
    parser.error('--bash-input-stress requires a positive count, --bash-smoke and --usb-storage')
if args.drag_stress and args.mouse_stress <= 0:
    parser.error('--drag-stress requires --mouse-stress')
if args.no_network and not args.bash_smoke:
    parser.error('--no-network requires --bash-smoke; the default test launches HTTP')
if args.bash_smoke and (args.unplug_root or args.duplicate_root):
    parser.error('--bash-smoke requires a stable, unique root disk')
if args.usb_storage and args.virtio_storage:
    parser.error('--usb-storage and --virtio-storage are mutually exclusive')
if args.unplug_root and not args.usb_storage:
    parser.error('--unplug-root requires --usb-storage')
if args.empty_sata and not args.usb_storage:
    parser.error('--empty-sata requires --usb-storage')
if args.late_root and (not args.usb_storage or args.empty_sata or args.duplicate_root or args.unplug_root):
    parser.error('--late-root requires --usb-storage without other root attachment scenarios')
if not args.image.is_file():
    parser.error('--image must be a regular image file, not a physical device')
repo = pathlib.Path(__file__).resolve().parents[2]
logs = pathlib.Path(tempfile.mkdtemp(prefix='nanami-usb-test-'))
base_image = logs / 'boot.img'
# Keep a stable backing file even if another build replaces its output. APFS
# clones share storage; other hosts use an ordinary private copy.
if sys.platform == 'darwin':
    subprocess.run(['cp', '-c', str(args.image.resolve()), str(base_image)], check=True)
else:
    shutil.copyfile(args.image, base_image)
if args.late_root:
    boot_without_root(base_image, logs / 'boot-only.img')
firmware = repo / 'spencer/a9nloader-rs/tools'
shutil.copyfile(firmware / 'OVMF_VARS.fd', logs / 'OVMF_VARS.fd')
serial = logs / 'serial.log'
qmp_path = logs / 'qmp.sock'
command = [
    'qemu-system-x86_64', '-machine', f'hpet={args.hpet},i8042={"on" if args.coexist_ps2 else "off"}',
    '-cpu', 'max', '-smp', str(args.smp), '-m', args.memory, '-display', 'none',
    *([] if args.high_mmio else ['-fw_cfg', 'name=opt/ovmf/X-PciMmio64Mb,string=0']),
    '-serial', f'file:{serial}', '-qmp', f'unix:{qmp_path},server=on,wait=off',
    *(['-nic', 'none'] if args.no_network else [
        '-netdev', 'user,id=net0,restrict=on',
        '-device', 'virtio-net,netdev=net0,addr=5,disable-legacy=off,disable-modern=on']),
    '-device', 'qemu-xhci,id=xhci,addr=6,msi=off,msix=off' + (',p3=0' if args.usb2 else ''),
    '-device', 'usb-kbd,id=usb-kbd,bus=xhci.0,port=1',
    '-device', 'usb-mouse,id=usb-mouse,bus=xhci.0,port=2',
    '-drive', f'if=pflash,format=raw,readonly=on,file={firmware / "OVMF_CODE.fd"}',
    '-drive', f'if=pflash,format=raw,file={logs / "OVMF_VARS.fd"}',
    *([] if args.virtio_storage else ['-device', 'ich9-ahci,id=ahci,addr=3']),
    '-drive', f'if=none,id=disk,format=raw,snapshot=on,file={base_image}',
    *(['-drive', f'if=none,id=boot-only,format=raw,snapshot=on,file={logs / "boot-only.img"}'] if args.late_root else []),
    '-device', ('ide-hd,drive=boot-only,bus=ahci.0,bootindex=1' if args.late_root
                else 'usb-storage,id=usb-root,drive=disk,bus=xhci.0,port=3,bootindex=1' if args.usb_storage
                else 'virtio-blk-pci,drive=disk,addr=3,bootindex=1,disable-legacy=off,disable-modern=on'
                if args.virtio_storage else 'ide-hd,drive=disk,bus=ahci.0,bootindex=1'),
    '--no-reboot', '--no-shutdown',
]
if args.mouse_stress:
    command += ['-trace', f'enable=usb_xhci_xfer_success,file={logs / "usb-transfers.log"}']
if args.duplicate_root:
    command += [
        '-drive', f'if=none,id=duplicate,format=raw,snapshot=on,file={base_image}',
        '-device', 'usb-storage,id=duplicate-root,drive=duplicate,bus=xhci.0,port=4',
    ]
if args.empty_sata:
    subprocess.run(['qemu-img', 'create', '-q', '-f', 'raw', str(logs / 'non-root.img'), '1M'], check=True)
    command += [
        '-drive', f'if=none,id=non-root,format=raw,snapshot=on,file={logs / "non-root.img"}',
        '-device', 'ide-hd,drive=non-root,bus=ahci.0',
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
            qmp('stop')
            qmp('screendump', {'filename': str(logs / 'timeout.png'), 'format': 'png'})
            diagnostics = {}
            for command in ['info registers -a', 'info irq', 'info usb', 'info mice', 'info pic', 'info lapic']:
                diagnostics[command] = qmp('human-monitor-command', {'command-line': command})
            for cpu in range(args.smp):
                diagnostics[f'cpu {cpu} lapic'] = qmp('human-monitor-command', {'command-line': 'info lapic', 'cpu-index': cpu})
            (logs / 'timeout-monitor.json').write_text(json.dumps(diagnostics, indent=2))
            raise RuntimeError('Guest timed out; inspect serial.log and timeout-monitor.json')

        qmp('qmp_capabilities')
        if args.late_root:
            wait_for(lambda s: 'usb-server] online controllers=' in s
                     and 'ahci-server] stopped status=' in s and s.count('boot-hid=1') >= 2)
            # Deliberately keep the root absent through the initial scan. This
            # is a test stimulus, not a timeout-based discovery policy in the OS.
            time.sleep(2)
            log = serial.read_text(errors='replace')
            assert 'storage=true' not in log
            assert 'service registered: block-device' not in log
            assert 'service registered: vfs-service' not in log
            qmp('device_add', {'driver': 'usb-storage', 'id': 'usb-root', 'drive': 'disk',
                               'bus': 'xhci.0', 'port': '3'})
            print('Attached USB root after the initial empty scan', flush=True)
        if args.duplicate_root:
            wait_for(lambda s: 'root probe/selection failed:' in s and 'usb-server] online controllers=' in s)
            log = serial.read_text(errors='replace')
            assert 'service registered: block-device' not in log
            assert 'service registered: vfs-service' not in log
            print('PASS: ambiguous root disks were rejected before block-device publication', flush=True)
            raise SystemExit(0)
        wait_for(lambda s: 'usb-server] online controllers=' in s
                 and s.count('boot-hid=1') >= 2 and 'font init end' in s and '[              honoka] online' in s)
        if args.usb_storage:
            log = serial.read_text(errors='replace')
            assert 'usb-server] service registered: block-device' in log
            assert 'ahci-server] service registered: block-device' not in log
            if args.late_root:
                assert log.count('usb-server] service registered: block-device') == 1
                assert 'usb-server] no root yet; watching storage connections' in log
                assert all(record['stats']['wr_bytes'] == 0 for record in qmp('query-blockstats')
                           if record.get('device') == 'boot-only'), 'Probe wrote to the firmware-only disk'
                print('PASS: late USB root published once and mounted without rebooting', flush=True)
            if args.empty_sata:
                assert all(record['stats']['wr_bytes'] == 0 for record in qmp('query-blockstats')
                           if record.get('device') == 'non-root'), 'Probe wrote to the non-root disk'
            print('USB BOT rootfs mounted; AHCI did not claim an empty controller', flush=True)
        print('USB keyboard/mouse enumerated; desktop ready', flush=True)
        print('Active pointers: ' + json.dumps(qmp('query-mice')), flush=True)
        time.sleep(5)
        if args.mouse_stress:
            qmp('screendump', {'filename': str(logs / 'before-stress.ppm')})
            _, dimensions, _, _ = (logs / 'before-stress.ppm').read_bytes().split(b'\n', 3)
            width, height = map(int, dimensions.split())
            qmp('input-send-event', {'events': [
                {'type': 'rel', 'data': {'axis': 'x', 'value': 300 - width * 3 // 4}},
                {'type': 'rel', 'data': {'axis': 'y', 'value': (90 if args.drag_stress else 200) - height // 2}},
            ]})
            time.sleep(2)
            if args.drag_stress:
                qmp('input-send-event', {'events': [{'type': 'btn', 'data': {'down': True, 'button': 'left'}}]})
                time.sleep(0.1)
            for index in range(args.mouse_stress):
                side = (index // 32) % 4
                qmp('input-send-event', {'events': [{'type': 'rel', 'data': {
                    'axis': 'x' if side % 2 == 0 else 'y',
                    'value': 4 if side < 2 else -4,
                }}]})
                time.sleep(0.005)
            if args.drag_stress:
                qmp('input-send-event', {'events': [{'type': 'btn', 'data': {'down': False, 'button': 'left'}}]})
                time.sleep(1)
            # Restore a known position even if overload/clipping dropped deltas.
            for x, y in [(-width * 2, -height * 2), (width * 3 // 4, height // 2)]:
                qmp('input-send-event', {'events': [
                    {'type': 'rel', 'data': {'axis': 'x', 'value': x}},
                    {'type': 'rel', 'data': {'axis': 'y', 'value': y}},
                ]})
                time.sleep(2)
            print(f'Injected {args.mouse_stress} mouse motions; checking keyboard, storage and clock next', flush=True)
        for _ in range(args.stress):
            for down in (True, False):
                qmp('input-send-event', {'events': [{'type': 'key', 'data': {
                    'down': down, 'key': {'type': 'qcode', 'data': 'shift'}}}]})
                time.sleep(0.05)
        # QEMU directs events to the USB keyboard/mouse. By default the VM has
        # no i8042 controller, so a PS/2 driver cannot make this test pass.
        def type_text(text):
            for char in text:
                key = {'-': 'minus', '\n': 'ret', ' ': 'spc', '/': 'slash', '.': 'dot'}.get(char, char)
                qmp('send-key', {'keys': [{'type': 'qcode', 'data': key}], 'hold-time': 50})
                time.sleep(0.8)

        if args.usb_storage:
            def root_written_bytes():
                records = qmp('query-blockstats')
                return sum(record['stats']['wr_bytes'] for record in records
                           if record.get('device') == 'disk' or record.get('qdev', '').endswith('/usb-root'))

            before = root_written_bytes()
            type_text('mkdir usbcheck\n')
            deadline = time.monotonic() + 20
            while root_written_bytes() <= before and time.monotonic() < deadline:
                time.sleep(0.2)
            assert root_written_bytes() > before, 'No filesystem writes reached the USB snapshot'
            print('Filesystem mkdir reached USB WRITE commands (snapshot only)', flush=True)

        if args.wheel_smoke:
            exercise_wheel(qmp, type_text, logs)

        if args.bash_input_stress:
            if args.doom_first:
                run_doom_first(qmp, type_text, wait_for, logs)
            exercise_bash_input(qmp, type_text, wait_for, serial, root_written_bytes, args.bash_input_stress)
            qmp('screendump', {'filename': str(logs / 'after-bash.png'), 'format': 'png'})
        elif args.bash_smoke:
            type_text('alter -t /alter/linux/bin/bash\n')
            wait_for(lambda s: re.search(r'managed rootfs process image=bash pid=(\d+)', s))
            bash_pid = re.search(r'managed rootfs process image=bash pid=(\d+)', serial.read_text(errors='replace'))[1]
            wait_for(lambda s: f'read stdin pid={bash_pid} ' in s)
            for iteration in range(3):
                start = len(serial.read_text(errors='replace'))
                type_text('/bin/busybox ls\n')
                wait_for(lambda s: re.search(r'exit pid=\d+ syscall=\d+ status=\d+\b', s[start:]))
                exited = re.search(r'exit pid=\d+ syscall=\d+ status=(\d+)\b', serial.read_text(errors='replace')[start:])
                assert exited[1] == '0', f'Bash child failed with status {exited[1]}'
                print(f'PASS: bash child {iteration + 1} exited successfully', flush=True)
            type_text('exit\n')
            wait_for(lambda s: re.search(r'exit pid=' + bash_pid + r' syscall=\d+ status=0\b', s))
            qmp('screendump', {'filename': str(logs / 'after-bash.png'), 'format': 'png'})
            print('PASS: interactive bash fork/exec/wait/exit over USB input', flush=True)
        else:
            type_text('http-server\n')
            qmp('screendump', {'filename': str(logs / 'after-keyboard.ppm')})
            wait_for(lambda s: 'tcp listen port=80' in s)
            print('USB keyboard launched HTTP', flush=True)
        if args.unplug_root:
            qmp('send-key', {'keys': [{'type': 'qcode', 'data': 'f12'}], 'hold-time': 100})
            time.sleep(2)
            qmp('device_del', {'id': 'usb-root'})
            wait_for(lambda s: 'usb-server] detached port=' in s)
            # QEMU removes the legacy drive backend with the USB device.
            # Supply a fresh overlay, never reopen a writable host base image.
            replacement = logs / 'replacement.qcow2'
            subprocess.run(['qemu-img', 'create', '-q', '-f', 'qcow2', '-F', 'raw',
                            '-b', str(base_image), str(replacement)], check=True)
            qmp('blockdev-add', {'driver': 'qcow2', 'node-name': 'replacement',
                                'file': {'driver': 'file', 'filename': str(replacement)}})
            qmp('device_add', {'driver': 'usb-storage', 'id': 'usb-root-new', 'drive': 'replacement',
                               'bus': 'xhci.0', 'port': '3'})
            wait_for(lambda s: s.count('storage=true') >= 2)
            def replacement_reads():
                nodes = [record for record in qmp('query-blockstats', {'query-nodes': True})
                         if record.get('node-name') == 'replacement']
                assert len(nodes) == 1
                return nodes[0]['stats']['rd_bytes']

            before = replacement_reads()
            type_text('eg-test\n')  # An executable not loaded during startup/HTTP.
            wait_for(lambda s: 'root device detached; I/O disabled' in s)
            after = replacement_reads()
            assert before == after, 'Old root handle accessed the replugged disk'
            print('PASS: unplug/replug did not redirect rootfs I/O to the replacement device', flush=True)
            raise SystemExit(0)
        time.sleep(2)
        qmp('screendump', {'filename': str(logs / 'before-mouse.ppm')})
        qmp('input-send-event', {'events': [
            {'type': 'rel', 'data': {'axis': 'x', 'value': 120}},
            {'type': 'rel', 'data': {'axis': 'y', 'value': 60}},
        ]})
        time.sleep(5 if args.bash_smoke else 1)
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
        if args.bash_smoke:
            if cursor_region(logs / 'before-mouse.ppm', width - 48, 4) == cursor_region(logs / 'after-mouse.ppm', width - 48, 4):
                raise RuntimeError('Desktop clock did not advance after bash exit')
            print('PASS: USB cursor and desktop clock still update after bash exit', flush=True)
            raise SystemExit(0)
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
    except Exception:
        # Assertions outside wait_for (e.g. input delivered but no disk write)
        # need the same stopped-CPU evidence as boot timeouts.
        if stream is not None and process.poll() is None:
            try:
                qmp('stop')
                qmp('screendump', {'filename': str(logs / 'failure.png'), 'format': 'png'})
                state = {}
                for monitor in ['info registers -a', 'info irq', 'info usb', 'info mice', 'info pic']:
                    state[monitor] = qmp('human-monitor-command', {'command-line': monitor})
                for cpu in range(args.smp):
                    state[f'cpu {cpu} lapic'] = qmp('human-monitor-command', {'command-line': 'info lapic', 'cpu-index': cpu})
                (logs / 'failure-monitor.json').write_text(json.dumps(state, indent=2))
            except Exception as diagnostic_error:
                print(f'Could not capture failure state: {diagnostic_error}', file=sys.stderr)
        raise
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
        base_image.unlink()
