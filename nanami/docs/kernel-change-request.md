# Capability Reclamation: Current Contract and Historical Requests

This document preserves the original kernel findings and records the current
integration boundary. The v0.3.8 observations below are historical; they must
not be used as descriptions of the corrected kernel.

## Current Status (2026-10-23)

- A9N `d9bd10df9703edae4e110b37fc7e632a35140cae` (v0.3.9-smp), through
  Spencer `628310ba6d31a4df7a8e7d9fd83b4d21a690f041`.
- ABI crate `a9n_abi` remains 0.8.0; the fixes change capability lifecycle
  semantics without adding Nanami-specific kernel operations.
- An authorized RW Node can REMOVE a contained WO, RO or NONE-rights alias.
  REVOKE still requires RW on the target because resetting the shared object is
  different from deleting one alias. DEMOTE cannot promote rights.
- COPY/MINT aliases are peers. Object identity does not include rights or IPC
  identifiers; changing an IPC identifier still requires MODIFY.
- The Node layer manages dependency traversal and slot cleanup. Each object
  owns its `revoke` behavior; Node teardown also drains contained references,
  including aliases of externally allocated objects.
- Recursive failure retains the remaining ownership needed for a safe retry;
  it does not promise atomic rollback. Generic allocation state must not reset
  before dependent cleanup succeeds.
- PCB revocation synchronously stops remote execution before teardown. Driver
  DMA quiescence and Nanami's reference/quota accounting remain separate.
- Port revoke handles queued IPC, not all accepted CALLs. An accepted caller
  may remain blocked waiting for its independent reply relationship; PCB
  teardown handles that process's reply links separately. No global process
  enumeration is needed for ordinary Port revoke.

The current implementation is in
[`capability_node.cpp`](../../spencer/A9N/src/kernel/capability/capability_node.cpp),
[`capability_node_reclamation.cpp`](../../spencer/A9N/src/kernel/capability/capability_node_reclamation.cpp),
[`capability_component.hpp`](../../spencer/A9N/src/kernel/include/kernel/capability/capability_component.hpp),
[`process_control_block.cpp`](../../spencer/A9N/src/kernel/capability/process_control_block.cpp)
and [`ipc_port.cpp`](../../spencer/A9N/src/kernel/capability/ipc_port.cpp).
Node cleanup and object teardown are distinct responsibilities; no separate
pre-revoke callback is required. Capability slots remain eight machine words.

Nanami's current managed close/retire paths still quarantine resources. The
kernel fixes resolve the generic mechanisms requested below, not full
Session-quota reuse, old-Provider reclamation or child-domain shutdown. See
[Nanami lifecycle integration](#nanami-lifecycle-integration) and the
[architecture gates](architecture.md#architecture-migration-gates).

## Historical Baseline (2026-10-16)

- A9N `7eff05714fd3405e29c0e599ac1037d674bb24f8` (v0.3.8).
- Spencer `2abc9b21c597fcf96d8990a9925863936a7ed01c`.
- ABI crate `a9n_abi` 0.8.0: Generic CONVERT; Node COPY, MINT, DEMOTE, MOVE,
  REMOVE and REVOKE; IPC IDENTIFY/RECEIVE.

The original findings were reproduced with those revisions on x86_64 and
AArch64 QEMU, each with 1 and 4 cores. Sections 1/2 below retain the observed
failure sequences and requested behavior from that baseline.

## Current Regression Probe

The optional probe is an ELF launched through the common Alpha launch contract
with its own 512-MiB partition. It receives only that partition's real Generic
authority, creates its own test objects, and keeps all backing reserved after
the test. It is not an alternative Alpha initializer or proof of domain
reclamation. Run the following from the repository root, using disposable image
paths. Existing rootfs contents are preserved by the build helper; use fresh
paths when validating newly built guest binaries.

```sh
python3 tests/alpha/build-domain.py --arch x86_64 --kernel-capabilities \
  --rootfs out/probe-rootfs.img --image out/probe.img
python3 tests/alpha/boot-smoke.py --arch x86_64 --image out/probe.img \
  --smp 1 --expect-domain --expect-kernel-capabilities --expect-handover
python3 tests/alpha/boot-smoke.py --arch x86_64 --image out/probe.img \
  --smp 4 --expect-domain --expect-kernel-capabilities --expect-handover
python3 tests/alpha/build-domain.py --arch aarch64 --kernel-capabilities \
  --rootfs out/probe-rootfs-arm.img --image out/probe-arm.img
python3 tests/alpha/boot-smoke.py --arch aarch64 --image out/probe-arm.img \
  --rootfs out/probe-rootfs-arm.img --smp 1 --expect-domain --expect-kernel-capabilities --expect-handover
python3 tests/alpha/boot-smoke.py --arch aarch64 --image out/probe-arm.img \
  --rootfs out/probe-rootfs-arm.img --smp 4 --expect-domain --expect-kernel-capabilities --expect-handover
```

The probe's assertions are in
[`tests/alpha/capabilities/src/main.rs`](../../tests/alpha/capabilities/src/main.rs).
The current runner requires both `CAPABILITY-PROBE-PASS` and
`CAPABILITY-PROBE-DONE`. It asserts peer-alias preservation, weak-alias deletion,
Node member drainage, 32 Generic-reuse iterations and 16 synchronous process
revoke/reuse iterations. A multi-core run checks that remote worker execution
has stopped before reuse. These assertions replace the original observation-only
probe; current sources are not a reproducer for the old failure expectations.
Passing does not reclaim the probe's outer partition or establish Nanami's
complete live-domain lifecycle.

Focused kernel checks are available from `spencer/A9N`:

```sh
sh test/host/run-capability-reclamation.sh
sh test/host/run-pcb-lifecycle.sh
sh test/host/run-smp-reclamation.sh
SANITIZE=1 sh test/host/run-pending-receive.sh
sh test/host/run-capability-lookup-codegen.sh
```

The code-generation check covers capability lookup, not a complete IPC latency
benchmark. Preserve a Release/LTO baseline when changing CALL/reply-receive hot
paths. Commands here describe validation procedures, not a claim that all guest
images were rerun for this documentation update.

## 1. Historical Request: Removing Attenuated Capabilities

### Required behavior

An administrator of a capability space must be able to withdraw and reclaim a
client's WRITE-only IPC alias without granting it READ/receive, COPY, or MODIFY
rights. Closing sessions, releasing their quotas and reclaiming their containing
domains must not depend on overgranting object rights to clients.

### Observed Sequence on v0.3.8

1. Keep a full-rights Node N and a full-rights endpoint P.
2. MINT an alias C of P into N with WRITE only. READ/RECEIVE, COPY and IDENTIFY
   are correctly denied through C.
3. REMOVE(N, C) returns `PermissionDenied` even though N has full rights.
4. REVOKE(N, C) and DEMOTE(N, C, NONE) return `IllegalOperation`.
5. MOVE C to an administrator-owned slot succeeds and withdraws the original
   handle, but REMOVE still cannot delete the moved alias.
6. Revoking a dedicated Generic containing P and these aliases fails while the
   attenuated alias remains. That alias is still live after the failure.

RW alias removal worked and left sibling aliases valid. COPY/MINT used sibling
derivation, not a private child subtree; revoking the original endpoint alias
did not selectively revoke its copies.

That implementation imposed target-slot READ **and** WRITE in Node REMOVE,
REVOKE and DEMOTE. `capability_slot::try_remove_and_init` imposed the same test,
including when invoked during ancestor reclamation. The corresponding current
source locations are:

- [`capability_node.cpp`](../../spencer/A9N/src/kernel/capability/capability_node.cpp):
  `operation_remove`, `operation_revoke`, `operation_demote`.
- [`capability_component.hpp`](../../spencer/A9N/src/kernel/include/kernel/capability/capability_component.hpp):
  slot identity and peer-alias operations.
- [`capability_node_reclamation.cpp`](../../spencer/A9N/src/kernel/capability/capability_node_reclamation.cpp):
  current `try_remove_and_init` delegation and Node-owned cleanup.

### Alternatives Considered at the Time

- COPY/MINT preserve or attenuate rights; they cannot promote C back to RW.
- Holding another full-rights alias did not bypass C's target-slot checks.
- MOVE safely removes client access but merely relocates the live capability.
- Giving clients RW would enable receiving from the public endpoint and is not
  an acceptable reclamation workaround.
- The current Nanami close path moves C to a bounded administrator quarantine.
  It retains the associated slot/quota charge; it never claims the reference or
  backing was reclaimed. This is safe withdrawal, not unlimited session churn.

### Minimal request and verification

Separate authority to manage/remove a slot from authority to invoke its object.
An appropriately authorized Node operation and ancestor reclamation must be
able to delete an attenuated alias, preserving sibling aliases and their
identifiers. Specify failure/partial-progress semantics for recursive revoke;
the Generic must not reset its allocation cursor or permit backing reuse while
live aliases remain.

No new service-specific operation or policy is requested. Operation numbers and
wire shapes can remain unchanged if this is corrected as management semantics.
Verify WO, RO and NONE aliases, nested/sibling derivation, pending requests, and
failed revocation without double allocation. Repeated session open/close must
release slots/quotas after successful deletion.

## 2. Historical Request: Node Reclamation and Contained Slots

### Required behavior and observation

Before a capability Node's storage is reused, all contained slots must be
detached safely from their dependency chains, including copies of objects
owned by other Generic extents. Node/Generic reclamation must not report a
container empty while those member references remain live.

The original probe created an independent Node containing full/RW endpoint
aliases. REVOKE of the Node returned success, but the member alias remained
usable: COPY from that member into another slot succeeded afterwards. The test
deliberately did not reuse the Node's storage. Dangling references on backing
reuse are a risk inferred from this retained membership, not an intentionally
triggered use-after-free experiment.

At that revision, `capability_node::revoke` removed only members satisfying
`slot.is_same_slot(self)`. Ordinary endpoint members did not match the Node.
See [`capability_node.cpp`](../../spencer/A9N/src/kernel/capability/capability_node.cpp),
and `is_same_slot` in
[`capability_component.hpp`](../../spencer/A9N/src/kernel/include/kernel/capability/capability_component.hpp).
Those symbol names describe the historical baseline; current identity checks
use `is_same_object` and do not limit Node drainage to matching members.

### Alternatives, request and impact

At that revision an explicit userland walk could remove known RW slots, but
could not complete for attenuated members (request 1). A complete safe walk also
needs a documented observable containment/cleanup contract; a Generic's derivation tree
alone is not the same as a Node's member set. Quietly retaining receive rights,
assuming every member was allocated from the Node's Generic, or reusing backing
after an incomplete walk does not satisfy the required ownership guarantees.

Provide either documented Node destruction that drains its member slots before
storage release, or a supported complete drain/reclamation procedure. It must
handle external copies, nested nodes, self/cyclic references, and attenuated
members, and must not destroy unrelated source endpoints. Returning failure
must preserve enough ownership information to prevent unsafe backing reuse.

This is capability lifecycle behavior, not a request to move names, Provider
selection, Registry or domain policy into A9N. Verify containment separately
from derivation and include Generic revoke/reuse after successful drainage.

## Nanami Lifecycle Integration

Managed Service endpoints are allocated from Alpha's own pool. Clients receive
WRITE-only session aliases; Providers receive separately removable RW aliases
without COPY/MODIFY. Cooperative quiesce finishes accepted CALLs before old
receive authority is withdrawn; commit installs the new receiver without
changing existing client handles. Normal data requests remain direct IPC.

Closed session aliases remain in bounded quarantine. Retired managed Providers
are stopped, but their Cspace/backing is quarantined rather than reaped/reused.
Unknown transfer rollback failures fault the binding and retain uncertain
resources. These measures are not substitutes for the final OS
reclamation guarantees. Safe old-Provider reclamation, repeated quota reuse,
and full child-domain reclamation remain open. Explicit cross-domain stateless
imports retain charged WRITE|COPY export aliases; the current Nanami path does
not delete/reuse them despite the kernel's corrected deletion support. Opt-in
local opaque session buffers now withdraw old mappings while the cooperative Provider is blocked,
retain backing/client VA, and install successor mappings before its receive
alias. The guest resolver observes a real fault on stale old access. This does
not reclaim the old Cspace, the WO handle alias or retained buffer backing.
The local event profile also withdraws the old PCB-held and visible Notification
aliases, retains client work handles and queue cursors, and seeds rescan before
control publication. Its guest test covers pending work through abort/commit
and an accepted CALL whose old-Provider reply precedes quiescence. It still does
not reclaim attenuated aliases/backing or migrate private heaps/device DMA.

## Handoff scope (2026-10-16)

At that handoff the user explicitly retained the no-Kernel-change constraint
and requested handoff with unmet conditions recorded. This is not acceptance of the full
resource-reclamation gates as passed. A9N/Nun/Spencer sources, ABI dependencies
and pinned revisions were unchanged at that point. The subsequent A9N
v0.3.9-smp integration supersedes that historical constraint and kernel baseline.

Unmet conditions:

- Safe reclamation of retired Provider Cspaces, closed attenuated aliases,
  retained buffer backing and their charged quotas.
- Cooperative Child Alpha shutdown/administration and a proven stop, authority
  withdrawal and reclaim sequence for a whole child domain. A remote Suspend
  request alone is not treated as observed stop completion.
- Reuse of Booter-only backing after proving every retained reference is gone.
  The Booter is stopped and reserved; this is not memory reclamation.

The handoff requested generic deletion/containment semantics in requests 1/2;
those mechanisms are now implemented in A9N. Nanami still needs explicit
cleanup/accounting and child shutdown integration, with repeated
open/close and child start/stop/reuse tests. Do not enable the internal allocator
release path merely because a task is marked exited or REVOKE returned success.

Verified handoff evidence is summarized in
[Architecture migration gates](architecture.md#architecture-migration-gates)
and the [native test record](../../tests/native-performance/README.md#event-handover-capability-provenance-and-boot-regressions-2026-10-16).
At that handoff the native suite passed 699 tests (7 timing tests ignored);
135 selected AddressSanitizer tests passed (1 timing test ignored), xtask passed
24 tests, and the C++ wire test passed AddressSanitizer/UBSan. Event/permission,
opaque-buffer and recursive-import guest fixtures passed x86_64/AArch64, each
with 1/4 cores. Normal virtio boot and the AHCI desktop workflow passed 1/4 cores;
USB boot passed 4 cores. These dated results are not a new kernel-revision
regression record or substitutes for the outstanding OS reclamation evidence.
