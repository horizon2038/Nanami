//! Production tree selector compared with the original snapshot-based scan.
#![allow(dead_code)]
type Word = usize;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ManagedProcess { pid: Word, tgid: Word, parent_pid: Word, payload: [u8; 2264] }
impl ManagedProcess {
    const EMPTY: Self = Self { pid: 0, tgid: 0, parent_pid: 0, payload: [0; 2264] };
}
struct Runtime { managed: Vec<ManagedProcess> }
impl Runtime {
    fn managed_process(&self, pid: Word) -> Option<&ManagedProcess> {
        self.managed.iter().find(|p| p.pid == pid)
    }
}
#[path = "../../nanami/servers/apps/alter/shared/src/common/state/process_tree.rs"]
mod production;

fn original(r: &Runtime, root: Word) -> Option<ManagedProcess> {
    if root == 0 { return None; }
    let mut best = ManagedProcess::EMPTY;
    let mut best_depth = 0;
    for entry in &r.managed {
        if entry.pid == 0 { continue; }
        let mut current = entry.pid;
        let mut depth = 0;
        let found = loop {
            if depth > r.managed.len() { break None; }
            if current == root { break Some(depth); }
            let Some(p) = r.managed_process(current) else { break None; };
            if p.pid != p.tgid { current = p.tgid; }
            else if p.parent_pid == 0 { break None; }
            else { current = p.parent_pid; }
            depth += 1;
        };
        if let Some(depth) = found {
            if best.pid == 0 || depth >= best_depth { best = *entry; best_depth = depth; }
        }
    }
    if best.pid == 0 { None } else { Some(best) }
}
fn process(pid: Word, tgid: Word, parent: Word, tag: u8) -> ManagedProcess {
    ManagedProcess { pid, tgid, parent_pid: parent, payload: [tag; 2264] }
}

#[test]
fn selection_matches_original_with_holes_cycles_missing_parents_and_threads() {
    let mut seed = 0x47e1b9u64;
    for _ in 0..4096 {
        let mut managed = Vec::new();
        for pid in 1..=16 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let v = (seed >> 24) as Word;
            managed.push(process(if v & 15 == 0 { 0 } else { pid },
                if v & 3 == 0 { (v >> 4) % 20 } else { pid },
                (v >> 9) % 20, v as u8));
        }
        let r = Runtime { managed };
        for root in 0..=20 {
            assert_eq!(r.deepest_process_in_tree(root), original(&r, root));
        }
    }
}

#[test]
fn later_equal_depth_wins_and_removal_walk_is_child_first_with_full_payload() {
    let mut r = Runtime { managed: vec![
        ManagedProcess::EMPTY, process(1, 1, 0, 11), process(2, 2, 1, 22),
        process(3, 3, 1, 33), process(4, 3, 99, 44), process(5, 5, 2, 55),
        process(6, 6, 99, 66),
    ] };
    let mut removed = Vec::new();
    while let Some(selected) = r.deepest_process_in_tree(1) {
        assert_eq!(Some(selected), original(&r, 1));
        assert_eq!(selected.payload, [(selected.pid * 11) as u8; 2264]);
        removed.push(selected.pid);
        let slot = r.managed.iter().position(|p| p.pid == selected.pid).unwrap();
        r.managed[slot] = ManagedProcess::EMPTY;
    }
    assert_eq!(removed, [5, 4, 3, 2, 1]);
    assert_eq!(r.managed[6].pid, 6);
    assert!(Runtime { managed: vec![] }.deepest_process_in_tree(1).is_none());
}
