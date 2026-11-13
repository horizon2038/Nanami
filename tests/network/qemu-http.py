#!/usr/bin/env python3
"""Exercise HTTP on a read-only QEMU snapshot, optionally injecting LAN noise.

All sockets are loopback-only; the guest has restricted user networking.
The injection port is a private QEMU hub peer, not a host/LAN raw socket.
"""
import argparse
import ipaddress
import os
import pathlib
import platform
import re
import select
import shutil
import socket
import struct
import subprocess
import tempfile
import threading
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--image', type=pathlib.Path, required=True)
parser.add_argument('--smp', type=int, default=4)
parser.add_argument('--nic', choices=('virtio', 'e1000e', 'igb'), default='virtio')
parser.add_argument('--spare-nic', choices=('e1000e', 'igb'),
                    help='add an earlier PCI NIC on an isolated, disconnected LAN')
parser.add_argument('--late-link', action='store_true',
                    help='with --spare-nic, connect the DHCP NIC only after desktop startup')
parser.add_argument('--shared-intx', action='store_true',
                    help='put xHCI on the Intel NIC PIRQ to exercise timer RX fallback')
parser.add_argument('-n', '--requests', type=int, default=1000)
parser.add_argument('-c', '--concurrency', type=int, default=100)
parser.add_argument('--multicast', action='store_true')
parser.add_argument('--boot-multicast', action='store_true',
                    help='inject LAN multicast during DHCP bootstrap as well as HTTP')
parser.add_argument('--subnet-prefix', type=int, choices=(23, 24, 25), default=24,
                    help='DHCP subnet prefix; verify the guest applies its mask')
parser.add_argument('--evict-arp', action='store_true', help='also fill the neighbor cache with 40 test peers')
parser.add_argument('--keep-alive', action='store_true')
parser.add_argument('--timeout', type=int, default=60)
parser.add_argument('--fix-slirp-backlog', action='store_true',
                    help='Darwin only: raise the QEMU host-forward listener backlog from 1 to SOMAXCONN')
args = parser.parse_args()
if args.shared_intx and args.nic == 'virtio':
    parser.error('--shared-intx requires an Intel NIC')
if args.spare_nic and args.nic == 'virtio':
    parser.error('--spare-nic requires an Intel primary NIC')
if args.late_link and not args.spare_nic:
    parser.error('--late-link requires --spare-nic')
if args.boot_multicast and not args.multicast:
    parser.error('--boot-multicast requires --multicast')
repo = pathlib.Path(__file__).resolve().parents[2]
logs = pathlib.Path(tempfile.mkdtemp(prefix='nanami-http-test-'))
firmware = repo / 'spencer/a9nloader-rs/tools'
shutil.copyfile(firmware / 'OVMF_VARS.fd', logs / 'OVMF_VARS.fd')
serial = logs / 'serial.log'
qemu_env = os.environ.copy()
if args.fix_slirp_backlog:
    if platform.system() != 'Darwin':
        parser.error('--fix-slirp-backlog is a Darwin-only test workaround')
    library = logs / 'listen-backlog.dylib'
    subprocess.run(['cc', '-dynamiclib', str(pathlib.Path(__file__).with_name('listen-backlog.c')),
                    '-o', str(library)], check=True)
    qemu_env['DYLD_INSERT_LIBRARIES'] = ':'.join(filter(None, [
        str(library), qemu_env.get('DYLD_INSERT_LIBRARIES'),
    ]))

def free_port(kind):
    with socket.socket(socket.AF_INET, kind) as probe:
        probe.bind(('127.0.0.1', 0))
        return probe.getsockname()[1]

port = free_port(socket.SOCK_STREAM)
inject_port = free_port(socket.SOCK_DGRAM)
peer = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
peer.bind(('127.0.0.1', 0))
peer.setblocking(False)
peer_port = peer.getsockname()[1]
command = [
    'qemu-system-x86_64', '-machine', 'hpet=on', '-cpu', 'max', '-smp', str(args.smp),
    '-m', '4G', '-display', 'none', '-serial', f'file:{serial}', '-monitor', 'stdio',
    '-netdev', f'user,id=user0,restrict=on,net=10.0.2.0/{args.subnet_prefix},dhcpstart=10.0.2.15,hostfwd=tcp:127.0.0.1:{port}-:80',
    '-netdev', 'hubport,id=user-port,hubid=0,netdev=user0',
    '-netdev', f'socket,id=inject,udp=127.0.0.1:{peer_port},localaddr=127.0.0.1:{inject_port}',
    '-netdev', 'hubport,id=inject-port,hubid=0,netdev=inject',
    '-netdev', 'hubport,id=net0,hubid=0',
    '-device', ('virtio-net,netdev=net0,addr=5,disable-legacy=off,disable-modern=on'
                if args.nic == 'virtio' else f'{args.nic},id=primary,netdev=net0,addr=5,mac=52:54:00:12:34:56'),
    '-object', f'filter-dump,id=dump,netdev=net0,file={logs / "net.pcap"}',
    '-drive', f'if=pflash,format=raw,readonly=on,file={firmware / "OVMF_CODE.fd"}',
    '-drive', f'if=pflash,format=raw,file={logs / "OVMF_VARS.fd"}',
    '-device', 'ich9-ahci,id=ahci,addr=3',
    '-drive', f'if=none,id=disk,format=raw,snapshot=on,file={args.image.resolve()}',
    '-device', 'ide-hd,drive=disk,bus=ahci.0,bootindex=1', '--no-reboot', '--no-shutdown',
]
if args.shared_intx:
    # Slots 5 and 9 share PIRQ A on i440fx. USB owns the IRQ before intel-net.
    command.extend(['-device', 'qemu-xhci,addr=9'])
if args.spare_nic:
    command.extend(['-netdev', 'hubport,id=spare-net,hubid=1',
                    '-device', f'{args.spare_nic},id=spare,netdev=spare-net,addr=4,mac=52:54:00:12:34:57'])

def multicast_frame():
    ip = bytearray(struct.pack('!BBHHHBBH4s4s', 0x45, 0, 28, 0, 0, 1, 17, 0,
                               bytes([10, 0, 2, 100]), bytes([224, 0, 0, 251])))
    checksum = sum(struct.unpack('!10H', ip))
    while checksum >> 16:
        checksum = (checksum & 0xffff) + (checksum >> 16)
    struct.pack_into('!H', ip, 10, checksum ^ 0xffff)
    return bytes.fromhex('01005e0000fb0200000000640800') + ip + struct.pack('!HHHH', 5353, 5353, 8, 0)

def neighbor_frame(host):
    mac = bytes([2, 0, 0, 0, 0, host])
    guest_mac = bytes.fromhex('525400123456')
    return (guest_mac + mac + b'\x08\x06' + struct.pack('!HHBBH', 1, 0x0800, 6, 4, 2)
            + mac + bytes([10, 0, 2, host]) + guest_mac + bytes([10, 0, 2, 15]))

stop = threading.Event()
inject = threading.Event()
if args.boot_multicast:
    inject.set()
arp_requests = 0

def network_peer():
    global arp_requests
    next_multicast = next_eviction = 0
    while not stop.is_set():
        now = time.monotonic()
        if inject.is_set() and args.multicast and now >= next_multicast:
            peer.sendto(multicast_frame(), ('127.0.0.1', inject_port))
            next_multicast = now + 0.01
        if inject.is_set() and args.evict_arp and now >= next_eviction:
            for host in range(100, 140):
                peer.sendto(neighbor_frame(host), ('127.0.0.1', inject_port))
            next_eviction = now + 0.5
        if select.select([peer], [], [], 0.005)[0]:
            frame = peer.recv(65536)
            if (len(frame) >= 42 and frame[12:14] == b'\x08\x06'
                    and frame[20:22] == b'\x00\x01' and frame[28:32] == bytes([10, 0, 2, 15])
                    and frame[38:42] == bytes([10, 0, 2, 2])):
                arp_requests += 1

def wait_for(predicate, timeout=120):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise RuntimeError('QEMU exited; see qemu.log')
        if serial.exists() and predicate(serial.read_text(errors='replace')):
            return
        time.sleep(0.2)
    raise RuntimeError('Guest timed out; see serial.log/screen.ppm')

def send(command):
    process.stdin.write(command.encode() + b'\n')
    process.stdin.flush()
    output = b''
    while b'(qemu)' not in output:
        if not select.select([process.stdout], [], [], 5)[0]:
            raise RuntimeError('Monitor timed out')
        chunk = os.read(process.stdout.fileno(), 4096)
        if not chunk:
            raise RuntimeError('Monitor closed')
        output += chunk

def check_dhcp_wire(capture):
    """Check actual guest TX, including BOOTP padding within IP/UDP lengths."""
    data = capture.read_bytes()
    if len(data) < 24 or data[:4] not in (b'\xd4\xc3\xb2\xa1', b'\xa1\xb2\xc3\xd4'):
        raise RuntimeError('Missing/unsupported Ethernet pcap')
    endian = '<' if data[:4] == b'\xd4\xc3\xb2\xa1' else '>'
    assert struct.unpack_from(endian + 'I', data, 20)[0] == 1
    cursor = 24
    types = set()
    while cursor + 16 <= len(data):
        length = struct.unpack_from(endian + 'I', data, cursor + 8)[0]
        cursor += 16
        frame = data[cursor:cursor + length]
        cursor += length
        if len(frame) < 42 or frame[12:14] != b'\x08\x00':
            continue
        ihl = (frame[14] & 15) * 4
        udp = 14 + ihl
        if ihl < 20 or len(frame) < udp + 8 or frame[23] != 17:
            continue
        if frame[udp:udp + 4] != struct.pack('!HH', 68, 67):
            continue
        total = struct.unpack_from('!H', frame, 16)[0]
        udp_length = struct.unpack_from('!H', frame, udp + 4)[0]
        assert len(frame) >= 14 + total and total == ihl + udp_length
        assert udp_length >= 308, 'DHCP UDP payload is shorter than BOOTP minimum'
        header_sum = sum(struct.unpack('!' + 'H' * (ihl // 2), frame[14:udp]))
        while header_sum >> 16:
            header_sum = (header_sum & 0xffff) + (header_sum >> 16)
        assert header_sum == 0xffff, 'Invalid outgoing DHCP IPv4 checksum'
        assert frame[:6] == b'\xff' * 6
        assert frame[26:34] == b'\0' * 4 + b'\xff' * 4
        payload = frame[udp + 8:udp + udp_length]
        assert payload[:3] == b'\x01\x01\x06'
        assert payload[28:34] == frame[6:12]
        assert payload[236:240] == bytes.fromhex('63825363')
        option = 240
        while option < len(payload):
            code = payload[option]
            option += 1
            if code == 0:
                continue
            if code == 255:
                assert not any(payload[option:]), 'Nonzero padding after DHCP END'
                break
            assert option < len(payload)
            size = payload[option]
            option += 1
            assert option + size <= len(payload)
            if code == 53:
                assert size == 1
                types.add(payload[option])
            option += size
        else:
            raise RuntimeError('DHCP options missing END')
    assert {1, 3}.issubset(types), 'Capture must contain both DISCOVER and REQUEST'
    print('DHCP wire checks passed: DISCOVER/REQUEST padding, lengths, MAC and IPv4 checksum', flush=True)

print(f'Logs: {logs}', flush=True)
with (logs / 'qemu.log').open('w') as diagnostics:
    process = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=diagnostics, env=qemu_env)
    worker = threading.Thread(target=network_peer)
    worker.start()
    try:
        if args.spare_nic:
            send('set_link spare off')
            if args.late_link:
                send('set_link primary off')
        # Font diagnostics can interleave with another CPU's output. They are
        # not a readiness protocol; actual HTTP listen/traffic is checked below.
        wait_for(lambda text: '[              honoka] online' in text)
        if args.late_link:
            time.sleep(1)
            send('set_link primary on')
        expected_mask = str(ipaddress.IPv4Network(f'10.0.2.0/{args.subnet_prefix}').netmask)
        # SMP boot logs can interleave characters from different services.
        # Only apply_lease logs this dotted mask token; do not require its
        # prefix and address to survive as one uninterrupted serial line.
        wait_for(lambda text: re.search(r'\bmask=' + re.escape(expected_mask) + r'\b', text))
        if args.shared_intx:
            wait_for(lambda text: 'rx=timer' in text)
        print(f'Guest applied DHCP mask {expected_mask}', flush=True)
        if args.spare_nic:
            wait_for(lambda text: 'ports=2' in text and 'selected port=1' in text)
        time.sleep(5)
        for char in 'http-server\n':
            send(f'sendkey { {"-": "minus", chr(10): "ret"}.get(char, char)} 50')
            time.sleep(0.8)
        send(f'screendump {logs / "screen.ppm"}')
        wait_for(lambda text: 'tcp listen port=80' in text)
        print(f'Running ab -n {args.requests} -c {args.concurrency}; multicast={args.multicast}, evict-arp={args.evict_arp}, slirp-backlog-fix={args.fix_slirp_backlog}', flush=True)
        inject.set()
        ab_command = ['ab', '-n', str(args.requests), '-c', str(args.concurrency), '-s', str(args.timeout)]
        if args.keep_alive:
            ab_command.append('-k')
        ab_command.append(f'http://127.0.0.1:{port}/')
        with (logs / 'ab.log').open('w') as output:
            started = time.monotonic()
            benchmark = subprocess.Popen(ab_command, stdout=output, stderr=subprocess.STDOUT)
            try:
                status = benchmark.wait(timeout=args.timeout + 30)
            except subprocess.TimeoutExpired:
                benchmark.terminate()
                status = benchmark.wait(timeout=5)
        inject.clear()
        report = (logs / 'ab.log').read_text()
        print(report, flush=True)
        print(f'Benchmark wall time: {time.monotonic() - started:.3f} seconds', flush=True)
        print(f'Guest ARP requests for HTTP next hop: {arp_requests}', flush=True)
        if status != 0 or not re.search(rf'Complete requests:\s+{args.requests}\b', report) or not re.search(r'Failed requests:\s+0\b', report):
            raise RuntimeError('HTTP requests did not all complete successfully')
    finally:
        stop.set()
        worker.join(timeout=5)
        peer.close()
        try:
            process.stdin.write(b'quit\n')
            process.stdin.flush()
        except (BrokenPipeError, OSError):
            pass
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.terminate()
            process.wait(timeout=5)

check_dhcp_wire(logs / 'net.pcap')
