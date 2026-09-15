# USB keyboard and mouse

The initial USB server supports USB 1.x/2.0 HID Boot Protocol keyboard and
three-button relative mouse interfaces attached directly to PCI xHCI root ports
on x86_64. Composite boot-HID devices are parsed interface by interface. Up to
four controllers, sixteen slots per controller, and four HID interfaces per
device are supported. This is not general USB or full HID report-descriptor
support.

## Run

```sh
USB_INPUT=on QEMU_HPET=on STORAGE_DEVICE=ahci make run
```

This adds `qemu-xhci` with MSI/MSI-X disabled, `usb-kbd` and `usb-mouse`, without
removing PS/2. The normal profile is unchanged when `USB_INPUT=off` (default).
Driver Manager detects the controller automatically on hardware; `USB_INPUT`
only configures emulated devices in the QEMU launcher.

Only controllers with a firmware-assigned memory BAR below 4 GiB are accepted.
Alpha currently materializes MMIO capability chunks between the source generic's
watermark and the requested address. OVMF's default 32-GiB xHCI BAR exhausts its
fixed directory. The launcher therefore sets `opt/ovmf/X-PciMmio64Mb=0` for this
profile. This is a firmware allocation choice, not hard-coded BAR relocation.
High BARs are rejected before any PCI writes or Alpha mapping requests. Sparse
high-MMIO support is separate work; no kernel or loader changes are included.

## Boundaries

- Driver Manager performs PCI discovery, BAR sizing/restoration, and MSI/MSI-X
  disabling before spawning hardware drivers. USB does not race other processes
  on the shared CF8/CFC latch. Resources are disclosed only to the boot-selected
  USB server PID. No new kernel interface is used.
- PCI code lives in `driver-manager/src/arch/x86_64/usb.rs`. USB platform mapping,
  controller initialization, enumeration, control transfers, DMA rings, bounded
  descriptor parsing, and boot report decoding have separate modules.
- xHCI BIOS ownership handoff, reset, DCBAA, scratchpad buffers, command/event
  rings, endpoint contexts, SET_CONFIGURATION, SET_PROTOCOL and SET_IDLE are
  handled in user space. USB 2 PSI speed mappings and 32/64-byte contexts are
  interpreted; USB 3-class devices are not configured.
- Input reports are translated to the existing PS/2-set-1-style key codes and
  shared input queues. Input-server registrations now retain each driver's
  event mask instead of overwriting the previous keyboard/mouse PID.
- INTx is used when its line is available. A 10-ms selected-timer notification
  also polls the event ring, including when IRQ ownership is unavailable. No
  direct hardware timer counter is exposed.
- A ring has one outstanding transfer per endpoint. DMA is reused only after
  matching completion; disconnected slots are reused after Disable Slot
  completes. Timeout/fatal paths retain mappings. Detach and failed transfers
  release held input state; multiple USB keyboards/mice have per-interface state
  and shared key/button reference counts.

Not implemented: external or integrated USB hubs, EHCI/OHCI/UHCI, SuperSpeed
devices, report-only/NKRO/vendor HID protocols, wheels/extra mouse buttons,
keyboard LED synchronization, USB storage, device suspend/resume, PCI hotplug,
MSI/MSI-X delivery, and AArch64 platform USB. A USB 2 boot keyboard plugged into
a USB 3 receptacle can work through that controller's USB 2 logical root port;
an intervening hub is still unsupported. Physical hardware, BIOS handoff,
nonzero scratchpad counts, custom PSI mappings and 64-byte contexts have not
been runtime-validated on hardware.

## Tests

Host tests compile production descriptor/HID/ring/PSI code, input registration
and distribution, and PCI preparation with mocked configuration I/O:

```sh
cargo test --release --manifest-path tests/native-performance/Cargo.toml \
  --test usb --test usb-pci --test input-distribution
RUSTFLAGS=-Zsanitizer=address cargo test \
  --manifest-path tests/native-performance/Cargo.toml \
  --target aarch64-apple-darwin --test usb --test usb-pci --test input-distribution
```

The tests cover cycle-bit wrap and link crossing, in-flight ownership, malformed
descriptors, composite interfaces, rollover/duplicate usages, signed movement,
button numbering, releases, nonstandard Speed IDs, BAR restoration on error,
message-interrupt disabling, high-BAR rejection, and simultaneous USB/PS/2
registrations. They do not model real DMA/cache behavior or timing.

After `make image`, run isolated QEMU snapshots:

```sh
python3 tests/usb/qemu-hid.py --image spencer/out/x86_64-pc99-release/spencer.img
python3 tests/usb/qemu-hid.py --image spencer/out/x86_64-pc99-release/spencer.img --smp 1 --stress 150
python3 tests/usb/qemu-hid.py --image spencer/out/x86_64-pc99-release/spencer.img --coexist-ps2
python3 tests/usb/qemu-hid.py --image spencer/out/x86_64-pc99-release/spencer.img --high-mmio
```

By default i8042 is absent, so PS/2 cannot make USB input tests pass. The test
launches HTTP from Shell using the USB keyboard, checks both expected cursor
regions after a mouse move (not just arbitrary screen changes), holds Shift
across unplug, and launches a lowercase command after replug. Shell's F12
shortcut terminates the foreground test application. The coexistence variant
also launches it with USB unplugged to exercise PS/2. `--stress` generates extra
modifier transitions before typing. Logs/screenshots and temporary firmware
variables are kept in a printed temporary directory. No real USB devices, disk
writes, LAN access or host forwarding are used.

The final mouse check moves to Shell's close button and clicks with the replugged
USB mouse. `--high-mmio` instead leaves OVMF's default high PCI hole enabled and
checks that USB is declined while storage, Shell and Honoka still boot.

### Validation (2026-09-15)

- Release host suite: 125 passed, 4 benchmark tests ignored.
- USB/PCI/input tests under AddressSanitizer: 26 passed.
- All target-selected Rust applications/services built for x86_64 and AArch64;
  AArch64 USB runtime is deliberately unavailable.
- QEMU 1 CPU, i8042 disabled: 150 modifier press/release pairs, keyboard command,
  expected cursor movement, Shift held across unplug, and command after replug.
- QEMU 4 CPUs, i8042 disabled: keyboard/mouse, held-Shift hotplug, and replugged
  mouse clicking Shell's close button.
- QEMU 4 CPUs with PS/2: keyboard commands before/after USB hotplug and while
  USB was absent; expected USB cursor movement and modifier release.
- QEMU 4 CPUs, default high BAR: refusal logged, Shell/Honoka booted normally.

No physical hardware validation is claimed. A9N and Spencer sources were not
changed. The x86_64 kernel ELF remained byte-identical (SHA-256
`bb98956fa845b877fc3067cbd7c23c9995f2d30f848b78e99455bc488a50bb5a`).

## References

Register layouts and initialization follow the
[Intel xHCI 1.2b specification](https://cdrdv2-public.intel.com/625472/625472_xHCI_Rev1_2b.pdf),
especially sections 4.3, 4.5, 4.9, 5, 6 and 7. Boot reports and class requests
follow the [USB HID 1.12 specification](https://www.usb.org/sites/default/files/hid1_12.pdf),
sections 7.2 and Appendix B. The firmware option is documented in
[OVMF runtime configuration](https://github.com/tianocore/edk2/blob/master/OvmfPkg/RUNTIME_CONFIG.md#platform-optovmfx-pcimmio64mb).
