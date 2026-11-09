"""Opt-in Linux process accounting; never attribute these totals to guest IPC."""
import os
from pathlib import Path
import time
from host_affinity import parse_vcpus


def parse_stat(text):
    # comm may contain both spaces and ')'; following stat fields cannot.
    pid, separator, _ = text.partition(" (")
    end = text.rfind(") ")
    if not separator or end < len(pid) + 2:
        raise ValueError("invalid process stat header")
    fields = text[end + 2:].split()  # first item is field 3 (state)
    if len(fields) < 41:
        raise ValueError("process stat lacks guest accounting")
    values = dict(pid=int(pid), start_ticks=int(fields[19]),
                  user_ticks=int(fields[11]), system_ticks=int(fields[12]),
                  guest_ticks=int(fields[40]))
    if any(value < 0 for value in values.values()):
        raise ValueError("negative process counter")
    if values["guest_ticks"] > values["user_ticks"]:
        raise ValueError("guest time exceeds inclusive user time")
    return values


def snapshot(pid):
    begin = time.monotonic_ns()
    try:
        values = parse_stat(Path(f"/proc/{pid}/stat").read_text())
        if values["pid"] != pid:
            raise ValueError("process identity mismatch")
        values.update(available=True, ticks_per_second=os.sysconf("SC_CLK_TCK"))
    except (OSError, ValueError) as error:
        values = dict(available=False, reason=type(error).__name__)
    values.update(read_begin_ns=begin, read_end_ns=time.monotonic_ns())
    return values


def interval(before, after):
    if not before.get("available") or not after.get("available"):
        return dict(available=False, reason="snapshot unavailable")
    if any(sample["guest_ticks"] > sample["user_ticks"] for sample in (before, after)):
        return dict(available=False, reason="inconsistent cumulative counters")
    for field in ("pid", "start_ticks", "ticks_per_second"):
        if before[field] != after[field]:
            return dict(available=False, reason="process/clock identity changed")
    hz = before["ticks_per_second"]
    if hz <= 0 or after["read_begin_ns"] < before["read_end_ns"]:
        return dict(available=False, reason="invalid observation bounds")
    delta = {key: after[key] - before[key]
             for key in ("user_ticks", "system_ticks", "guest_ticks")}
    if any(value < 0 for value in delta.values()) or delta["guest_ticks"] > delta["user_ticks"]:
        return dict(available=False, reason="inconsistent process counters")
    # proc_pid_stat(5): utime already INCLUDES guest_time. Never add it again.
    return dict(available=True, ticks_per_second=hz, delta=delta,
                cpu_seconds=(delta["user_ticks"] + delta["system_ticks"]) / hz,
                guest_seconds=delta["guest_ticks"] / hz,
                user_excluding_guest_seconds=(delta["user_ticks"] - delta["guest_ticks"]) / hz,
                system_seconds=delta["system_ticks"] / hz,
                wall_lower_ns=after["read_begin_ns"] - before["read_end_ns"],
                wall_upper_ns=after["read_end_ns"] - before["read_begin_ns"])


def parse_schedstat(text):
    fields = text.split()
    if len(fields) != 3:
        raise ValueError("invalid scheduler counter fields")
    values = [int(field) for field in fields]
    if any(value < 0 or value >= 1 << 64 for value in values):
        raise ValueError("scheduler counter out of range")
    return dict(runtime_ns=values[0], runnable_ns=values[1], slices=values[2])


def thread_snapshot(pid, cpu_text, count):
    """Raw Linux scheduler counters, not validated physical CPU consumption."""
    result = dict(pid=pid, read_begin_ns=time.monotonic_ns(), available=False,
                  threads={}, cpu_cost_admitted=False, runnable_delay_available=False)
    try:
        vcpus = parse_vcpus(cpu_text.decode(errors="strict"), count)
        tids = sorted(int(path.name) for path in Path(f"/proc/{pid}/task").iterdir())
        if pid in vcpus or not ({pid} | set(vcpus)).issubset(tids):
            raise ValueError("vCPU thread does not belong to QEMU")
        result["vcpu_tids"] = vcpus
        enable = Path("/proc/sys/kernel/sched_schedstats")
        result["schedstats_enablement"] = enable.read_text().strip() if enable.exists() else None
        result["runnable_delay_available"] = result["schedstats_enablement"] == "1"
        complete = True
        for tid in tids:
            entry = dict(read_begin_ns=time.monotonic_ns(), available=False,
                         role=f"vcpu{vcpus.index(tid)}" if tid in vcpus else "main" if tid == pid else "other")
            result["threads"][str(tid)] = entry
            try:
                base = Path(f"/proc/{pid}/task/{tid}")
                entry["stat_raw"] = (base / "stat").read_text()
                entry["schedstat_raw"] = (base / "schedstat").read_text()
                identity = parse_stat(entry["stat_raw"])
                if identity["pid"] != tid:
                    raise ValueError("thread identity mismatch")
                entry.update(start_ticks=identity["start_ticks"],
                             scheduler=parse_schedstat(entry["schedstat_raw"]), available=True)
            except (OSError, ValueError) as error:
                entry["reason"] = str(error)
                complete = False
            entry["read_end_ns"] = time.monotonic_ns()
        result["available"] = complete
        if not complete:
            result["reason"] = "thread counters incomplete or inconsistent"
    except (OSError, ValueError) as error:
        result["reason"] = str(error)
    result["read_end_ns"] = time.monotonic_ns()
    return result


def thread_interval(before, after, process_interval):
    # Never bypass a failed inclusive utime/guest accounting check using a
    # plausible positive scheduler delta from a different counter domain.
    if not process_interval.get("available"):
        return dict(available=False, reason="process accounting unavailable/inconsistent")
    if not before.get("available") or not after.get("available"):
        return dict(available=False, reason="thread counters unavailable/inconsistent")
    if (before["pid"] != after["pid"] or before["vcpu_tids"] != after["vcpu_tids"]
            or set(before["threads"]) != set(after["threads"])):
        return dict(available=False, reason="QEMU thread set changed")
    rows = {}
    delay_available = before["runnable_delay_available"] and after["runnable_delay_available"]
    for tid, first in before["threads"].items():
        last = after["threads"][tid]
        if (not first.get("available") or not last.get("available")
                or first["start_ticks"] != last["start_ticks"] or first["role"] != last["role"]
                or first["read_end_ns"] < first["read_begin_ns"]
                or last["read_end_ns"] < last["read_begin_ns"]
                or last["read_begin_ns"] < first["read_end_ns"]):
            return dict(available=False, reason="thread identity or observation bounds changed")
        delta = {key: last["scheduler"][key] - first["scheduler"][key]
                 for key in ["runtime_ns", "runnable_ns", "slices"]}
        if any(value < 0 for value in delta.values()):
            return dict(available=False, reason="scheduler counters regressed")
        row = dict(role=first["role"], reported_runtime_ns=delta["runtime_ns"],
                   reported_slices=delta["slices"],
                   wall_lower_ns=last["read_begin_ns"] - first["read_end_ns"],
                   wall_upper_ns=last["read_end_ns"] - first["read_begin_ns"])
        if delay_available:
            row["reported_runnable_ns"] = delta["runnable_ns"]
        rows[tid] = row
    return dict(available=True, threads=rows, runnable_delay_available=delay_available,
                reported_runtime_sum_ns=sum(row["reported_runtime_ns"] for row in rows.values()),
                cpu_cost_admitted=False, domain="Linux-reported scheduler runtime; not physical CPU time")
