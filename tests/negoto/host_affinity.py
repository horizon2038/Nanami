"""Explicit per-vCPU placement for distinct Linux KVM diagnostic conditions."""
import os
from pathlib import Path
import re


def parse_vcpus(text, count):
    found = {}
    for cpu, tid in re.findall(r"(?m)^\s*\*?\s*CPU #(\d+): thread_id=(\d+)(?:\s|$)", text):
        cpu, tid = int(cpu), int(tid)
        if cpu in found and found[cpu] != tid:
            raise ValueError("conflicting QEMU vCPU thread IDs")
        found[cpu] = tid
    if set(found) != set(range(count)) or len(set(found.values())) != count:
        raise ValueError("missing/duplicate QEMU vCPU threads")
    return [found[cpu] for cpu in range(count)]


def thread_ids(pid):
    return {int(path.name) for path in Path(f"/proc/{pid}/task").iterdir()}


def expected_threads(pid, vcpus, cpus, io_cpu, tids):
    if len(vcpus) != len(cpus) or len(set(vcpus)) != len(vcpus) or pid in vcpus:
        raise ValueError("invalid vCPU identities")
    if len(set(cpus + [io_cpu])) != len(cpus) + 1:
        raise ValueError("vCPU and I/O CPUs must be distinct")
    if not ({pid} | set(vcpus)).issubset(tids):
        raise ValueError("vCPU thread is not in owning QEMU process")
    result = {tid: io_cpu for tid in tids}
    result.update(zip(vcpus, cpus))
    return result


def verify(pid, vcpus, cpus, io_cpu):
    expected = expected_threads(pid, vcpus, cpus, io_cpu, thread_ids(pid))
    observed = {tid: sorted(os.sched_getaffinity(tid)) for tid in expected}
    if any(mask != [expected[tid]] for tid, mask in observed.items()):
        raise RuntimeError("QEMU thread lost explicit CPU placement")
    return dict(vcpu_threads=vcpus, vcpu_cpus=cpus, io_cpu=io_cpu,
                expected=expected, observed=observed)


def apply(pid, vcpus, cpus, io_cpu):
    expected = expected_threads(pid, vcpus, cpus, io_cpu, thread_ids(pid))
    # Pin parent/auxiliary threads first, so later main-loop workers inherit I/O.
    for tid in sorted(expected, key=lambda tid: tid in vcpus):
        os.sched_setaffinity(tid, {expected[tid]})
    return verify(pid, vcpus, cpus, io_cpu)
