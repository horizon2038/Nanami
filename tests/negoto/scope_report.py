"""Validate the known scope fixture only; never admit a performance profiler."""
import re


def validate_checked_fixture(text, enabled):
    """Trust the hashed native fixture's binary assertions, not debug-log rows."""
    reports = 2 if enabled else 0
    footer = f"SYSCALL-SCOPE-FIXTURE-PASS cleanup=true enabled={int(enabled)} readout={reports}"
    if not re.search(r"(?m)^" + re.escape(footer) + r"\r?\n", text):
        raise ValueError("missing complete checked-readout/native-validation footer")
    if "syscall.scope.row" in text or "ledger-valid=" in text:
        raise ValueError("unexpected legacy row transport")
    return dict(schema=2, enabled=enabled, functional=True, performance_admitted=False,
                report_transport="owner-checked-packet/native-assertions",
                validated_reports=reports, counter_identity="local-unverified")


def validate_fixture(text, enabled):
    text = re.sub(r"\x1b\[[0-9;]*m", "", text)
    footer = f"SYSCALL-SCOPE-FIXTURE-PASS cleanup=true enabled={int(enabled)}"
    if not re.search(r"(?m)^" + re.escape(footer) + r"\r?\n", text):
        raise ValueError("missing complete native fixture/cleanup footer")
    scopes, ends, aborts = {}, set(), []
    current = None
    for line in text.splitlines():
        if "syscall.scope" not in line:
            continue
        header = re.fullmatch(r"\[\s*syscall\.scope\] owner=(\d+) epoch=(\d+) mode=(Count|Time) start=(\d+) end=(\d+) ledger-valid=(true|false) clock=local-unverified", line)
        row = re.fullmatch(r"\[\s*syscall\.scope\.row\] epoch=(\d+) syscall=(\d+) calls=(\d+) ticks=(\d+)", line)
        end = re.fullmatch(r"\[\s*syscall\.scope\.end\] epoch=(\d+)", line)
        abort = re.fullmatch(r"\[\s*syscall\.scope\.abort\] owner=(\d+) reason=(exit|exec|retire)", line)
        if header:
            owner, epoch, mode, start, stop, valid = header.groups()
            owner, epoch, start, stop = map(int, (owner, epoch, start, stop))
            if current is not None or epoch in scopes or owner == 0 or valid != "true" or stop < start:
                raise ValueError("invalid/duplicate/overlapping scope header")
            scopes[epoch] = dict(owner=owner, mode=mode, start=start, end=stop, rows={})
            current = epoch
        elif row:
            epoch, number, calls, ticks = map(int, row.groups())
            if epoch != current or number > 512 or calls == 0 or number in scopes[epoch]["rows"]:
                raise ValueError("orphan/duplicate/out-of-range scope row")
            scopes[epoch]["rows"][number] = dict(calls=calls, ticks=ticks)
        elif end:
            epoch = int(end[1])
            if epoch != current or epoch in ends:
                raise ValueError("unpaired scope footer")
            ends.add(epoch)
            current = None
        elif abort:
            if current is not None:
                raise ValueError("abort inside report")
            aborts.append(dict(owner=int(abort[1]), reason=abort[2]))
        else:
            raise ValueError("malformed/interleaved scope record")
    if not enabled:
        if scopes or aborts:
            raise ValueError("inactive image emitted scope records")
    else:
        if current is not None or set(scopes) != {1, 2} or ends != {1, 2}:
            raise ValueError("missing complete count/time reports")
        count, timed = scopes[1], scopes[2]
        if count["mode"] != "Count" or timed["mode"] != "Time" or count["owner"] != timed["owner"]:
            raise ValueError("unexpected fixture scope identities")
        expected = {39: 7, 57: 1, 60: 1, 61: 1, 157: 2, 512: 1}
        if count["rows"] != {number: dict(calls=calls, ticks=0) for number, calls in expected.items()}:
            raise ValueError("count stream differs from known fixture")
        if set(timed["rows"]) != {39} or timed["rows"][39]["calls"] != 5:
            raise ValueError("timed stream differs from known fixture")
        ticks = timed["rows"][39]["ticks"]
        if not 0 < ticks <= timed["end"] - timed["start"]:
            raise ValueError("timed subtotal outside its scope")
        if [abort["reason"] for abort in aborts] != ["exit", "exec"]:
            raise ValueError("missing owner exit/exec retirement")
        owners = [abort["owner"] for abort in aborts]
        if len(set(owners)) != 2 or count["owner"] in owners or 0 in owners:
            raise ValueError("invalid retired owners")
    return dict(schema=1, enabled=enabled, functional=True, performance_admitted=False,
                scopes=scopes, aborts=aborts)
