# IPv4 configuration and HTTP serving

Nanami has one active IPv4 interface. Intel providers can expose several
physical ports; the network service selects a port with carrier. This is not
simultaneous multi-interface routing.

## Configuration

`network-service` becomes available after the NIC backend is connected, without
waiting for DHCP. Acquisition is an event-driven state machine, so settings
remain accessible when no DHCP server answers. Failure leaves the address
unconfigured; it does not invent a QEMU-specific fallback address.

Run these commands in the native shell:

```text
net-control show
net-control manual 192.168.10.101 255.255.255.0 192.168.10.1 192.168.10.1
net-control dhcp
```

The last manual argument is an optional DNS server. Use `0.0.0.0` for no
gateway or DNS. The two mode commands above save and immediately apply a
complete configuration. For separate edits:

```text
net-control mode manual
net-control address 192.168.10.42
net-control netmask 255.255.255.0
net-control gateway 192.168.10.1
net-control dns 192.168.10.1
net-control reload
```

Field/mode edits save a draft without disturbing active traffic. `show` reports
both the saved draft and active configuration/state. `reload` validates the
complete saved configuration before changing anything. Applying closes existing
TCP flows and clears learned neighbors, queued packets and pending DNS work;
listening port ownership and client SHM attachments are retained. Applications
must reconnect their outgoing connections.

Saved settings survive reboot in `/nanami/network.settings`. This is a bounded
binary file with two checksummed, versioned records, not a text configuration.
Each successful save writes the inactive record, fsyncs, then verifies it.
A torn record does not replace the preceding valid record. This does not turn
ext2 into a journaled filesystem or guarantee recovery after arbitrary storage
failure. Malformed records and incomplete manual drafts are not silently
applied at startup.

The SDK's `net_service_settings`, `net_service_save_settings`,
`net_service_reload` and `net_service_config_state` provide the same operations.
Save requires the revision returned by read; stale revisions return
`NET_SERVICE_RESPONSE_CONFIG_CONFLICT` instead of overwriting another editor.
The additive UAPI leaves existing IPv4 configuration queries unchanged.

## Network Manager GUI

Launch `network-manager` from the native shell. This is a native Honoka window,
not a wrapper invoking `net-control`. Both clients use the same service API.

- Choose Automatic (DHCP) or Manual; manual mode enables IPv4 fields.
- Save & apply validates, persists and applies the form. Ctrl+S is equivalent.
- Reload (F5) applies the saved settings and discards local edits.
- Refresh reads the latest saved draft without applying it; use it after a
  revision conflict with another GUI/CLI editor.
- Tab changes fields; Ctrl+A selects a value. Ctrl+W closes the window.

The active address and acquisition state update independently of unsaved form
edits. Invalid input is reported in the window without applying it.

## HTTP server

```text
http-server
http-server --port 8080 --file /www/example.html
http-server --port 8080 --file /www/example.html --css /www/example.css
```

Defaults are port 80, `/www/index.html` and `/www/style.css`. The rootfs builder
installs the default HTML/CSS as ordinary files. Put custom files on the guest
filesystem before starting the server. Startup loads and validates regular
files once (HTML plus CSS at most 4 MiB); requests use immutable cached content
and prebuilt headers, with no per-request filesystem I/O. Restart the process
to reload changed files. A missing file fails startup rather than serving
embedded fallback content. Port must be between 1 and 65535.

GET/HEAD serve `/` and `/index.html` from the selected HTML file and `/style.css`
from the selected stylesheet; other paths return 404. Request headers may span
TCP reads. HTTP/1.0/1.1 persistence and bounded pipelining are supported. Header
limits are 8 KiB per request and 16 KiB pending per active response slot (128
slots). Requests with bodies/Transfer-Encoding, invalid framing or duplicate
Host/Content-Length fields are rejected and closed. This is a small static
server, not a general application or TLS server.

For QEMU user networking, forward the selected guest port explicitly, e.g.
`cargo xtask run --hostfwd tcp:127.0.0.1:8080-:8080`. A bridged/hardware guest
uses its configured LAN address directly.

## Throughput changes and validation

Intel multi-port providers now resolve PCI bridge INTx swizzling before using
ACPI root routing. Routable ports use interrupt-driven RX even when several
NICs exist; unavailable/shared routes retain a 2-ms fallback. I211/igb uses
EITR[0], while legacy devices use ITR, with a 32.768-us moderation interval.
See the [Intel provider documentation](../servers/core-services/intel-net/README.md).

A full TX ring returns retryable backpressure. TCP_SEND reports an accepted
prefix if a later segment cannot be submitted; HTTP advances by exactly that
prefix and does not repeat data or FIN. Deferred responses retry after 1 ms
instead of the preceding 10-ms wait. Ordinary idle still waits for RX events.
A short RX batch now ends the drain immediately instead of issuing another
IPC solely to reconfirm that the queue is empty. Full batches still drain up
to the budget, and later packets use the existing RX notification/timer path.

Host and QEMU regression commands are documented in
[network tests](../../tests/network/README.md). Virtual throughput is not a
measurement of the physical I219/I211 system; repeat the same request count,
concurrency and keep-alive policy on that system to compare the reported
approximately 50 rps. The existing stack's outgoing TCP retransmission and
congestion-control limitations are not resolved by these changes.
