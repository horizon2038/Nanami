//! Recycled metadata keeps process/reference index churn and heap traffic bounded.
use super::ProcessPhysicalAllocation;
use alloc::vec::Vec;

#[derive(Default)]
pub(super) struct PhysicalAllocations {
    by_pid: Index<usize>,
    references: Index<(usize, usize)>,
    nodes: Vec<Node>,
    free: Option<usize>,
}

#[derive(Clone, Copy)]
struct Node {
    allocation: ProcessPhysicalAllocation,
    next: Option<usize>,
}

impl PhysicalAllocations {
    pub(super) fn insert(&mut self, pid: usize, allocation: ProcessPhysicalAllocation) {
        let key = Self::key(allocation);
        let count = self.references.get(key).unwrap_or(0);
        self.references.insert(key, count + 1);
        let node = Node {
            allocation,
            next: self.by_pid.get(pid),
        };
        let index = if let Some(free) = self.free {
            self.free = self.nodes[free].next;
            self.nodes[free] = node;
            free
        } else {
            self.nodes.push(node);
            self.nodes.len() - 1
        };
        self.by_pid.insert(pid, index);
    }

    pub(super) fn for_pid(
        &self,
        pid: usize,
    ) -> impl Iterator<Item = ProcessPhysicalAllocation> + '_ {
        let mut next = self.by_pid.get(pid);
        core::iter::from_fn(move || {
            let node = self.nodes[next?];
            next = node.next;
            Some(node.allocation)
        })
    }

    pub(super) fn count(&self, allocation: ProcessPhysicalAllocation) -> usize {
        self.references.get(Self::key(allocation)).unwrap_or(0)
    }

    pub(super) fn find(
        &self,
        pid: usize,
        base_va: usize,
        page_count: usize,
    ) -> Option<ProcessPhysicalAllocation> {
        self.for_pid(pid)
            .find(|allocation| allocation.base_va == base_va && allocation.page_count == page_count)
    }

    pub(super) fn take(
        &mut self,
        pid: usize,
        base_va: usize,
        page_count: usize,
    ) -> Option<(ProcessPhysicalAllocation, bool)> {
        let mut next = self.by_pid.get(pid);
        let mut previous: Option<usize> = None;
        while let Some(index) = next {
            let node = self.nodes[index];
            if node.allocation.base_va == base_va && node.allocation.page_count == page_count {
                if let Some(previous) = previous {
                    self.nodes[previous].next = node.next;
                } else if let Some(head) = node.next {
                    self.by_pid.insert(pid, head);
                } else {
                    self.by_pid.remove(pid);
                }
                return Some(self.recycle(index));
            }
            previous = Some(index);
            next = node.next;
        }
        None
    }

    pub(super) fn remove_pid(
        &mut self,
        pid: usize,
        mut removed: impl FnMut(ProcessPhysicalAllocation, bool),
    ) {
        let mut next = self.by_pid.remove(pid);
        while let Some(index) = next {
            next = self.nodes[index].next;
            let (allocation, last) = self.recycle(index);
            removed(allocation, last);
        }
    }

    fn key(allocation: ProcessPhysicalAllocation) -> (usize, usize) {
        (allocation.base_page, allocation.page_count)
    }

    fn recycle(&mut self, index: usize) -> (ProcessPhysicalAllocation, bool) {
        let allocation = self.nodes[index].allocation;
        let key = Self::key(allocation);
        let count = self
            .references
            .get(key)
            .expect("registered physical allocation");
        if count == 1 {
            self.references.remove(key);
        } else {
            self.references.insert(key, count - 1);
        }
        self.nodes[index].next = self.free;
        self.free = Some(index);
        (allocation, count == 1)
    }
}

trait Key: Copy + Eq {
    fn hash(self) -> usize;
}
fn mix(mut value: u64) -> usize {
    value ^= value >> 30;
    value = value.wrapping_mul(0xbf58476d1ce4e5b9);
    value ^= value >> 27;
    value = value.wrapping_mul(0x94d049bb133111eb);
    (value ^ (value >> 31)) as usize
}
impl Key for usize {
    fn hash(self) -> usize {
        mix(self as u64)
    }
}
impl Key for (usize, usize) {
    fn hash(self) -> usize {
        mix((self.0 as u64).wrapping_add((self.1 as u64).wrapping_mul(0x9e3779b97f4a7c15)))
    }
}

// At most half full. Backward-shift deletion leaves no tombstones, so repeated
// PID/range reuse does not lengthen probes or allocate replacement tree nodes.
// Growth is geometric; all ordinary insert/remove operations reuse storage.
#[derive(Default)]
struct Index<K> {
    slots: Vec<Option<(K, usize)>>,
    len: usize,
}
impl<K: Key> Index<K> {
    fn locate(&self, key: K) -> Option<usize> {
        if self.slots.is_empty() {
            return None;
        }
        let mask = self.slots.len() - 1;
        let mut index = key.hash() & mask;
        loop {
            match self.slots[index] {
                Some((found, _)) if found == key => return Some(index),
                Some(_) => index = (index + 1) & mask,
                None => return None,
            }
        }
    }
    fn get(&self, key: K) -> Option<usize> {
        self.locate(key).map(|index| self.slots[index].unwrap().1)
    }
    fn insert(&mut self, key: K, value: usize) {
        if let Some(index) = self.locate(key) {
            self.slots[index] = Some((key, value));
            return;
        }
        if (self.len + 1) * 2 > self.slots.len() {
            let capacity = self.slots.len().saturating_mul(2).max(16);
            let old = core::mem::replace(&mut self.slots, alloc::vec![None; capacity]);
            self.len = 0;
            for (key, value) in old.into_iter().flatten() {
                self.insert(key, value);
            }
        }
        let mask = self.slots.len() - 1;
        let mut index = key.hash() & mask;
        while self.slots[index].is_some() {
            index = (index + 1) & mask;
        }
        self.slots[index] = Some((key, value));
        self.len += 1;
    }
    fn remove(&mut self, key: K) -> Option<usize> {
        let mut hole = self.locate(key)?;
        let (_, value) = self.slots[hole].take().unwrap();
        self.len -= 1;
        let mask = self.slots.len() - 1;
        let mut scan = (hole + 1) & mask;
        while let Some((key, _)) = self.slots[scan] {
            let ideal = key.hash() & mask;
            if (scan.wrapping_sub(ideal) & mask) >= (scan.wrapping_sub(hole) & mask) {
                self.slots[hole] = self.slots[scan].take();
                hole = scan;
            }
            scan = (scan + 1) & mask;
        }
        Some(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Clone, Copy, Default, PartialEq, Eq)]
    struct Colliding(usize);
    impl Key for Colliding {
        fn hash(self) -> usize {
            15
        }
    }

    #[test]
    fn collision_clusters_wrap_delete_and_reinsert_without_tombstones() {
        let mut index = Index::<Colliding>::default();
        for key in 0..256 {
            index.insert(Colliding(key), key + 1000);
        }
        for key in (0..256).step_by(3) {
            assert_eq!(index.remove(Colliding(key)), Some(key + 1000));
        }
        for key in 0..256 {
            assert_eq!(
                index.get(Colliding(key)),
                (key % 3 != 0).then_some(key + 1000)
            );
        }
        for key in 0..256 {
            index.insert(Colliding(key), key + 2000);
        }
        for key in (0..256).rev() {
            assert_eq!(index.remove(Colliding(key)), Some(key + 2000));
        }
        assert_eq!(index.len, 0);
        assert!(index.slots.iter().all(Option::is_none));
    }

    #[test]
    fn repeated_pid_and_range_churn_reuses_all_metadata_storage() {
        let mut allocations = PhysicalAllocations::default();
        for iteration in 0..10000 {
            let value = ProcessPhysicalAllocation {
                base_va: 4096,
                start_slot: 1,
                base_page: iteration + 1,
                page_count: 1,
            };
            allocations.insert(iteration + 1, value);
            assert_eq!(allocations.count(value), 1);
            allocations.remove_pid(iteration + 1, |_, last| assert!(last));
            assert_eq!(allocations.nodes.len(), 1);
            assert_eq!(allocations.by_pid.slots.len(), 16);
            assert_eq!(allocations.references.slots.len(), 16);
            assert_eq!(allocations.references.len, 0);
        }
    }

    #[test]
    fn randomized_index_matches_ordered_map() {
        let mut index = Index::<usize>::default();
        let mut reference = alloc::collections::BTreeMap::new();
        let mut seed = 73u64;
        for step in 0..30000 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let key = (seed >> 19) as usize % 4096;
            if seed % 3 == 0 {
                assert_eq!(index.remove(key), reference.remove(&key));
            } else {
                index.insert(key, step);
                reference.insert(key, step);
            }
            assert_eq!(index.len, reference.len());
            assert_eq!(index.get(key), reference.get(&key).copied());
        }
        for key in 0..4096 {
            assert_eq!(index.get(key), reference.get(&key).copied());
        }
    }
}
