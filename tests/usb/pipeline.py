"""No mouse events, guest writes or extra guest RPCs; observe existing trace output."""
import re
import time

STAGES = ('usb-publish', 'input-distribute', 'honoka-pop', 'honoka-route', 'shell-pop')


def exercise(qmp, serial, logs):
    before = serial.read_text(errors='replace')
    if not re.search(r'\[\s*pipe\.clock\]', before):
        raise RuntimeError('Rebuild with NANAMI_PIPELINE_TRACE=1 before --pipeline-smoke')
    qmp('screendump', {'filename': str(logs / 'pipeline-before.ppm')})
    started = time.monotonic()
    qmp('send-key', {'keys': [{'type': 'qcode', 'data': 'a'}], 'hold-time': 80})
    deadline = started + 4
    while time.monotonic() < deadline:
        text = serial.read_text(errors='replace')[len(before):]
        records = {stage: re.findall(r'\[\s*pipe\.key\]\s+stage=' + stage
                   + r'\s+batch=\d+ total=(\d+)[^\r\n]*notify-errors=(\d+)', text) for stage in STAGES}
        found = {stage for stage, rows in records.items() if any(int(total) >= 2 for total, _ in rows)}
        shell_badges = re.findall(r'\[\s*pipe\.wake\]\s+stage=shell-pop[^\r\n]*badge=(0x[0-9a-f]+)', text)
        if found == set(STAGES) and any(int(badge, 16) & (1 << 49) for badge in shell_badges):
            if any(int(errors) for rows in records.values() for _, errors in rows) or re.search(r'\[\s*pipe\.notify\]', text):
                raise RuntimeError(f'Input notification failed; inspect {serial}')
            qmp('screendump', {'filename': str(logs / 'pipeline-after.ppm')})
            print(f'PASS: key press/release reached all five stages in {time.monotonic() - started:.3f}s without mouse input; Shell woke on its input badge', flush=True)
            return
        time.sleep(0.05)
    missing = sorted(set(STAGES) - found)
    raise RuntimeError(f'Input pipeline stalled or serial records interleaved; missing {missing}. Inspect {serial}')
