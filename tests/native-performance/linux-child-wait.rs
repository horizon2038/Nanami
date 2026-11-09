//! Production child-event selection against the original ordered scan.
#![allow(dead_code)]
type Word = usize;
#[derive(Clone, Copy, Default)]
struct Job { group: Word }
#[derive(Clone, Copy, Default)]
struct Signals { exit_signal: Word, child_event: Option<u32> }
#[derive(Clone, Copy, Default)]
struct Process {
    pid: Word, tgid: Word, parent_pid: Word, job: Job, exited: bool,
    exit_status: Word, state: Option<Signals>,
}
impl Process { fn signals(&self) -> Option<&Signals> { self.state.as_ref() } }
struct Runtime { managed: Vec<Process> }
impl Runtime {
    fn managed_process(&self, pid: Word) -> Option<&Process> {
        self.managed.iter().find(|p| p.pid == pid)
    }
    fn thread_group(&self, pid: Word) -> Word {
        self.managed_process(pid).map_or(pid, |p| p.tgid)
    }
}
fn wait_target_matches(pid: Word, target: Word, child_group: Word, group: Word) -> bool {
    if target == Word::MAX { true }
    else if target == 0 { child_group == group }
    else if (target as isize) < 0 { (target as isize).checked_neg().is_some_and(|v| v as Word == child_group) }
    else { target == pid }
}
#[path = "../../nanami/servers/apps/alter/shared/src/common/state/child_wait.rs"]
mod production;

fn original(r: &Runtime, parent: Word, target: Word, stops: bool, continues: bool) -> Option<(Word,u32,bool)> {
    let parent = r.thread_group(parent);
    let group = r.managed_process(parent)?.job.group;
    for p in &r.managed {
        if p.pid == 0 || p.pid != p.tgid
            || r.managed.iter().any(|t| t.pid != p.pid && !t.exited && t.tgid == p.tgid)
            || p.parent_pid != parent || !wait_target_matches(p.pid,target,p.job.group,group) { continue; }
        if p.exited {
            let signal = p.signals().map_or(0, |s| s.exit_signal);
            return Some((p.pid, if signal == 0 { ((p.exit_status & 255) << 8) as u32 } else { signal as u32 & 127 }, true));
        }
        if let Some(status) = p.signals().and_then(|s| s.child_event) {
            if (status == 65535 && continues) || (status != 65535 && stops) { return Some((p.pid,status,false)); }
        }
    }
    None
}

#[test]
fn ordered_events_match_original_with_threads_targets_and_report_masks() {
    let mut seed = 0x713ad84u64;
    for _ in 0..4096 {
        let mut rows = vec![Process { pid:1,tgid:1,job:Job{group:3},..Process::default() }];
        for pid in 2..=16 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let v = seed >> 24;
            rows.push(Process {
                pid: if v & 31 == 0 {0} else {pid}, tgid: if v & 3 == 0 {2} else {pid},
                parent_pid: if v & 4 == 0 {1} else {9}, job:Job{group:(v as Word >> 4)%5},
                exited:v & 8 != 0, exit_status:v as Word,
                state: if v & 16 == 0 {None} else {Some(Signals {
                    exit_signal: if v & 32 == 0 {0} else {v as Word & 255},
                    child_event: match v % 3 {0=>None,1=>Some(65535),_=>Some(0x137f)},
                })},
            });
        }
        let r = Runtime { managed: rows };
        for parent in [1,2,16,99] {
            for target in [Word::MAX,0,2,9,16,99,(-3isize) as Word,isize::MIN as Word] {
                for flags in 0..4 {
                    assert_eq!(r.child_wait_event(parent,target,flags&1!=0,flags&2!=0), original(&r,parent,target,flags&1!=0,flags&2!=0));
                }
            }
        }
    }
}

#[test]
fn unrelated_rows_and_eventless_children_do_not_require_thread_discovery() {
    let mut r = Runtime { managed:(1..=16).map(|pid| Process {
        pid,tgid:pid,parent_pid:42,..Process::default()
    }).collect() };
    production::take_thread_checks();
    assert_eq!(r.child_wait_event(1,Word::MAX,true,true),None);
    let unrelated = production::take_thread_checks();
    for p in &mut r.managed { p.parent_pid=1; }
    assert_eq!(r.child_wait_event(1,Word::MAX,true,true),None);
    let eventless = production::take_thread_checks();
    r.managed[15].exited=true; r.managed[15].exit_status=7;
    assert_eq!(r.child_wait_event(1,16,true,true),Some((16,7<<8,true)));
    let selected = production::take_thread_checks();
    println!("CHILD-WAIT-THREAD-CHECKS unrelated={unrelated} eventless={eventless} selected={selected}");
}
