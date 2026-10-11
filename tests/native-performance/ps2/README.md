# PS/2 initialization waits

The rootfs PS/2 driver runs at priority 28; ext2 runs at priority 22. On the
reported four-core hardware both were on core 3. Initialization previously
polled I/O up to 200,000 times for a writable controller, or 500,000 times for
configuration/data/ACK. Those loops never blocked. Their duration depended on
I/O latency, and lower-priority work on the same core could not run meanwhile.

The driver now rejects an all-ones status before attaching to input-service.
Unready hardware is polled with blocking 1 ms sleeps and elapsed-time deadlines
(200 ms for input-buffer clearance, 500 ms for a reply). Immediate success does
not query the clock or sleep. Timer completion uses a separate notification so
keyboard/mouse IRQs cannot wake a delay early. Timeouts log the stage and status.

These tests include the actual initialization, controller, decoder and wait
modules. Mock I/O/clock/sleep checks cover absent and stuck controllers, missing
replies, unrelated keyboard traffic, optional mouse-ID failure, coarse PIT time,
clock wrap and transport failure. An excessive busy-poll count fails the test.
They establish the polling defect, not the cause of every real-hardware hang.

```sh
cargo test --release --manifest-path tests/native-performance/Cargo.toml --test ps2-init
```

For a separate test disk containing the rebuilt init ELF **and rootfs**:

```sh
python3 tests/acpi/qemu-shutdown.py --image /path/to/test.img --storage usb --usb-input --i8042 off
python3 tests/acpi/qemu-shutdown.py --image /path/to/test.img --storage usb --usb-input --i8042 on
```

The USB-storage configuration includes an empty AHCI controller, matching the
reported driver spawn order/core assignments. Tests use QEMU disk snapshots and
require desktop startup followed by Shell `shutdown` and a guest ACPI S5 event.
QEMU still supplies xHCI INTx, unlike the reported polling-only real controller.
