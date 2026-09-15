#!/usr/bin/env python3
"""Exercise HTTP on a read-only QEMU snapshot, optionally injecting LAN noise.

All sockets are loopback-only; the guest has restricted user networking.
The injection port is a private QEMU hub peer, not a host/LAN raw socket.
"""
import argparse
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
parser.add_argument('-n', '--requests', type=int, default=1000)
parser.add_argument('-c', '--concurrency', type=int, default=100)
parser.add_argument('--multicast', action='store_true')
parser.add_argument('--evict-arp', action='store_true', help='also fill the neighbor cache with 40 test peers')
parser.add_argument('--keep-alive', action='store_true')
parser.add_argument('--timeout', type=int, default=60)
parser.add_argument('--fix-slirp-backlog', action='store_true',
                    help='Darwin only: raise the QEMU host-forward listener backlog from 1 to SOMAXCONN')
args = parser.parse_args()
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
    '-netdev', f'user,id=user0,restrict=on,hostfwd=tcp:127.0.0.1:{port}-:80',
    '-netdev', 'hubport,id=user-port,hubid=0,netdev=user0',
    '-netdev', f'socket,id=inject,udp=127.0.0.1:{peer_port},localaddr=127.0.0.1:{inject_port}',
    '-netdev', 'hubport,id=inject-port,hubid=0,netdev=inject',
    '-netdev', 'hubport,id=net0,hubid=0',
    '-device', 'virtio-net,netdev=net0,addr=5,disable-legacy=off,disable-modern=on',
    '-object', f'filter-dump,id=dump,netdev=net0,file={logs / "net.pcap"}',
    '-drive', f'if=pflash,format=raw,readonly=on,file={firmware / "OVMF_CODE.fd"}',
    '-drive', f'if=pflash,format=raw,file={logs / "OVMF_VARS.fd"}',
    '-device', 'ich9-ahci,id=ahci,addr=3',
    '-drive', f'if=none,id=disk,format=raw,snapshot=on,file={args.image.resolve()}',
    '-device', 'ide-hd,drive=disk,bus=ahci.0,bootindex=1', '--no-reboot', '--no-shutdown',
]

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

print(f'Logs: {logs}', flush=True)
with (logs / 'qemu.log').open('w') as diagnostics:
    process = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=diagnostics, env=qemu_env)
    worker = threading.Thread(target=network_peer)
    worker.start()
    try:
        wait_for(lambda text: re.search(r'\[\s*shell\] font init end', text) and '[              honoka] online' in text)
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
