"""Count real Doom save-game I/O on the harness's disposable USB snapshot."""
import json
import time


def exercise(qmp, logs):
    def key(code):
        qmp('send-key', {'keys': [{'type': 'qcode', 'data': code}], 'hold-time': 100})

    def stats():
        records = [r['stats'] for r in qmp('query-blockstats')
                   if r.get('device') == 'disk' or r.get('qdev', '').endswith('/usb-root')]
        assert len(records) == 1, records
        return records[0]

    # Attract demo -> New Game -> episode 1 -> default difficulty.
    key('esc')
    time.sleep(1)
    for _ in range(3):
        key('ret')
        time.sleep(1)
    time.sleep(3)
    qmp('screendump', {'filename': str(logs / 'doom-before-save.png'), 'format': 'png'})
    key('f2')
    time.sleep(1)
    key('ret')
    time.sleep(1)
    for char in 'usbtest':
        key(char)
        time.sleep(0.2)
    before = stats()
    start = time.monotonic()
    key('ret')
    last_change = start
    previous = before
    while time.monotonic() - start < 60:
        time.sleep(0.1)
        after = stats()
        if after['wr_operations'] != previous['wr_operations']:
            last_change = time.monotonic()
        previous = after
        if after['wr_operations'] > before['wr_operations'] and time.monotonic() - last_change > 1:
            break
    else:
        raise AssertionError('Doom save produced no completed, quiescent USB writes')
    result = {field: after[field] - before[field] for field in
              ('rd_operations', 'wr_operations', 'flush_operations', 'rd_bytes', 'wr_bytes')}
    # This is the observed disk-activity interval, not a guest syscall latency.
    result['observed_write_interval_seconds'] = round(last_change - start, 3)
    (logs / 'doom-save.json').write_text(json.dumps(result, indent=2) + '\n')
    qmp('screendump', {'filename': str(logs / 'doom-after-save.png'), 'format': 'png'})
    print('Doom save USB I/O: ' + json.dumps(result), flush=True)
    # Load the saved slot through Doom itself, then retain a screenshot for
    # checking that the save/load menus have returned to gameplay.
    key('f3')
    time.sleep(1)
    qmp('screendump', {'filename': str(logs / 'doom-load-menu.png'), 'format': 'png'})
    key('ret')
    time.sleep(5)
    qmp('screendump', {'filename': str(logs / 'doom-after-load.png'), 'format': 'png'})
