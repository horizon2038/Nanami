use alloc::boxed::Box;

const PAGE_BITS: usize = 12;
const PAGE_SIZE: usize = 1 << PAGE_BITS;

#[derive(Clone, Copy)]
pub struct PhysicalAllocation {
    pub base_page: usize,
    pub page_count: usize,
    pub is_device: bool,
}

#[derive(Clone, Copy)]
pub enum PhysicalAllocError {
    InvalidArgument,
    PermissionDenied,
    OutOfMemory,
}

#[derive(Clone, Copy)]
struct Region {
    base_page: usize,
    page_count: usize,
    is_device: bool,
    used: bool,
}

struct Node {
    region: Region,
    left: Option<Box<Node>>,
    right: Option<Box<Node>>,
    height: i16,
}

impl Node {
    fn new(region: Region) -> Box<Self> {
        Box::new(Self {
            region,
            left: None,
            right: None,
            height: 1,
        })
    }
}

struct RegionTree {
    root: Option<Box<Node>>,
}

impl RegionTree {
    const fn new() -> Self {
        Self { root: None }
    }

    fn height(node: &Option<Box<Node>>) -> i16 {
        node.as_ref().map(|n| n.height).unwrap_or(0)
    }

    fn recompute_height(node: &mut Box<Node>) {
        let lh = Self::height(&node.left);
        let rh = Self::height(&node.right);
        node.height = if lh > rh { lh + 1 } else { rh + 1 };
    }

    fn balance_factor(node: &Box<Node>) -> i16 {
        Self::height(&node.left) - Self::height(&node.right)
    }

    fn rotate_left(mut x: Box<Node>) -> Box<Node> {
        let mut y = x.right.take().expect("rotate_left requires right child");
        x.right = y.left.take();
        Self::recompute_height(&mut x);
        y.left = Some(x);
        Self::recompute_height(&mut y);
        y
    }

    fn rotate_right(mut y: Box<Node>) -> Box<Node> {
        let mut x = y.left.take().expect("rotate_right requires left child");
        y.left = x.right.take();
        Self::recompute_height(&mut y);
        x.right = Some(y);
        Self::recompute_height(&mut x);
        x
    }

    fn rebalance(mut node: Box<Node>) -> Box<Node> {
        Self::recompute_height(&mut node);
        let bf = Self::balance_factor(&node);

        if bf > 1 {
            if Self::balance_factor(node.left.as_ref().unwrap()) < 0 {
                let left = node.left.take().unwrap();
                node.left = Some(Self::rotate_left(left));
            }
            return Self::rotate_right(node);
        }

        if bf < -1 {
            if Self::balance_factor(node.right.as_ref().unwrap()) > 0 {
                let right = node.right.take().unwrap();
                node.right = Some(Self::rotate_right(right));
            }
            return Self::rotate_left(node);
        }

        node
    }

    fn insert(&mut self, region: Region) {
        self.root = Some(Self::insert_node(self.root.take(), region));
    }

    fn insert_node(node: Option<Box<Node>>, region: Region) -> Box<Node> {
        let Some(mut n) = node else {
            return Node::new(region);
        };

        if region.base_page < n.region.base_page {
            n.left = Some(Self::insert_node(n.left.take(), region));
        } else if region.base_page > n.region.base_page {
            n.right = Some(Self::insert_node(n.right.take(), region));
        } else {
            n.region = region;
            return n;
        }

        Self::rebalance(n)
    }

    fn remove(&mut self, base_page: usize) -> Option<Region> {
        let (new_root, removed) = Self::remove_node(self.root.take(), base_page);
        self.root = new_root;
        removed
    }

    fn remove_node(
        node: Option<Box<Node>>,
        base_page: usize,
    ) -> (Option<Box<Node>>, Option<Region>) {
        let Some(mut n) = node else {
            return (None, None);
        };

        if base_page < n.region.base_page {
            let (left, removed) = Self::remove_node(n.left.take(), base_page);
            n.left = left;
            return (Some(Self::rebalance(n)), removed);
        }
        if base_page > n.region.base_page {
            let (right, removed) = Self::remove_node(n.right.take(), base_page);
            n.right = right;
            return (Some(Self::rebalance(n)), removed);
        }

        let removed = Some(n.region);
        match (n.left.take(), n.right.take()) {
            (None, None) => (None, removed),
            (Some(left), None) => (Some(left), removed),
            (None, Some(right)) => (Some(right), removed),
            (Some(left), Some(right)) => {
                let (new_right, successor) = Self::extract_min(right);
                let mut new_node = successor;
                new_node.left = Some(left);
                new_node.right = new_right;
                (Some(Self::rebalance(new_node)), removed)
            }
        }
    }

    fn extract_min(mut node: Box<Node>) -> (Option<Box<Node>>, Box<Node>) {
        match node.left.take() {
            None => (node.right.take(), node),
            Some(left) => {
                let (new_left, min_node) = Self::extract_min(left);
                node.left = new_left;
                (Some(Self::rebalance(node)), min_node)
            }
        }
    }

    fn get(&self, base_page: usize) -> Option<Region> {
        let mut cursor = self.root.as_ref();
        while let Some(n) = cursor {
            if base_page < n.region.base_page {
                cursor = n.left.as_ref();
            } else if base_page > n.region.base_page {
                cursor = n.right.as_ref();
            } else {
                return Some(n.region);
            }
        }
        None
    }

    fn find_containing(&self, base_page: usize, page_count: usize) -> Option<Region> {
        let mut cursor = self.root.as_ref();
        while let Some(n) = cursor {
            let start = n.region.base_page;
            let end = start + n.region.page_count;
            if base_page < start {
                cursor = n.left.as_ref();
            } else if base_page >= end {
                cursor = n.right.as_ref();
            } else {
                let req_end = base_page + page_count;
                if req_end <= end {
                    return Some(n.region);
                }
                return None;
            }
        }
        None
    }

    fn first_fit_non_device(&self, page_count: usize) -> Option<Region> {
        Self::first_fit_in_node(&self.root, page_count)
    }

    fn first_fit_in_node(node: &Option<Box<Node>>, page_count: usize) -> Option<Region> {
        let Some(n) = node.as_ref() else {
            return None;
        };

        if let Some(r) = Self::first_fit_in_node(&n.left, page_count) {
            return Some(r);
        }

        if !n.region.used && !n.region.is_device && n.region.page_count >= page_count {
            return Some(n.region);
        }

        Self::first_fit_in_node(&n.right, page_count)
    }

    fn predecessor_key(&self, base_page: usize) -> Option<usize> {
        let mut cursor = self.root.as_ref();
        let mut best = None;
        while let Some(n) = cursor {
            if base_page <= n.region.base_page {
                cursor = n.left.as_ref();
            } else {
                best = Some(n.region.base_page);
                cursor = n.right.as_ref();
            }
        }
        best
    }

    fn successor_key(&self, base_page: usize) -> Option<usize> {
        let mut cursor = self.root.as_ref();
        let mut best = None;
        while let Some(n) = cursor {
            if base_page < n.region.base_page {
                best = Some(n.region.base_page);
                cursor = n.left.as_ref();
            } else {
                cursor = n.right.as_ref();
            }
        }
        best
    }
}

pub struct PhysicalAllocator {
    tree: RegionTree,
}

impl PhysicalAllocator {
    pub fn new() -> Self {
        Self {
            tree: RegionTree::new(),
        }
    }

    pub fn add_region(
        &mut self,
        base_address: usize,
        size_bytes: usize,
        is_device: bool,
        used: bool,
    ) -> Result<(), PhysicalAllocError> {
        if !is_page_aligned(base_address) || size_bytes == 0 {
            return Err(PhysicalAllocError::InvalidArgument);
        }
        let page_count = bytes_to_pages(size_bytes);
        if page_count == 0 {
            return Err(PhysicalAllocError::InvalidArgument);
        }
        self.tree.insert(Region {
            base_page: base_address >> PAGE_BITS,
            page_count,
            is_device,
            used,
        });
        Ok(())
    }

    pub fn allocate_at(
        &mut self,
        base_address: usize,
        size_bytes: usize,
        allow_device: bool,
    ) -> Result<PhysicalAllocation, PhysicalAllocError> {
        if !is_page_aligned(base_address) || size_bytes == 0 {
            return Err(PhysicalAllocError::InvalidArgument);
        }
        let req_base = base_address >> PAGE_BITS;
        let req_count = bytes_to_pages(size_bytes);

        let container = self
            .tree
            .find_containing(req_base, req_count)
            .ok_or(PhysicalAllocError::OutOfMemory)?;

        if container.used {
            return Err(PhysicalAllocError::OutOfMemory);
        }
        if container.is_device && !allow_device {
            return Err(PhysicalAllocError::PermissionDenied);
        }

        self.tree.remove(container.base_page);

        if req_base > container.base_page {
            self.tree.insert(Region {
                base_page: container.base_page,
                page_count: req_base - container.base_page,
                is_device: container.is_device,
                used: false,
            });
        }

        self.tree.insert(Region {
            base_page: req_base,
            page_count: req_count,
            is_device: container.is_device,
            used: true,
        });

        let container_end = container.base_page + container.page_count;
        let req_end = req_base + req_count;
        if req_end < container_end {
            self.tree.insert(Region {
                base_page: req_end,
                page_count: container_end - req_end,
                is_device: container.is_device,
                used: false,
            });
        }

        Ok(PhysicalAllocation {
            base_page: req_base,
            page_count: req_count,
            is_device: container.is_device,
        })
    }

    pub fn allocate_any(
        &mut self,
        size_bytes: usize,
    ) -> Result<PhysicalAllocation, PhysicalAllocError> {
        if size_bytes == 0 {
            return Err(PhysicalAllocError::InvalidArgument);
        }
        let req_count = bytes_to_pages(size_bytes);
        let region = self
            .tree
            .first_fit_non_device(req_count)
            .ok_or(PhysicalAllocError::OutOfMemory)?;

        self.allocate_at(region.base_page << PAGE_BITS, req_count << PAGE_BITS, false)
    }

    pub fn free(
        &mut self,
        base_address: usize,
        size_bytes: usize,
    ) -> Result<(), PhysicalAllocError> {
        if !is_page_aligned(base_address) || size_bytes == 0 {
            return Err(PhysicalAllocError::InvalidArgument);
        }

        let req_base = base_address >> PAGE_BITS;
        let req_count = bytes_to_pages(size_bytes);

        let container = self
            .tree
            .find_containing(req_base, req_count)
            .ok_or(PhysicalAllocError::InvalidArgument)?;

        if !container.used {
            return Err(PhysicalAllocError::InvalidArgument);
        }

        self.tree.remove(container.base_page);

        if req_base > container.base_page {
            self.tree.insert(Region {
                base_page: container.base_page,
                page_count: req_base - container.base_page,
                is_device: container.is_device,
                used: true,
            });
        }

        self.tree.insert(Region {
            base_page: req_base,
            page_count: req_count,
            is_device: container.is_device,
            used: false,
        });

        let container_end = container.base_page + container.page_count;
        let req_end = req_base + req_count;
        if req_end < container_end {
            self.tree.insert(Region {
                base_page: req_end,
                page_count: container_end - req_end,
                is_device: container.is_device,
                used: true,
            });
        }

        self.merge_neighbors(req_base)?;
        Ok(())
    }

    fn merge_neighbors(&mut self, base_page: usize) -> Result<(), PhysicalAllocError> {
        let mut center = self
            .tree
            .get(base_page)
            .ok_or(PhysicalAllocError::InvalidArgument)?;
        if center.used {
            return Ok(());
        }

        if let Some(prev_key) = self.tree.predecessor_key(center.base_page) {
            if let Some(prev) = self.tree.get(prev_key) {
                if !prev.used
                    && prev.is_device == center.is_device
                    && prev.base_page + prev.page_count == center.base_page
                {
                    self.tree.remove(prev.base_page);
                    self.tree.remove(center.base_page);
                    center = Region {
                        base_page: prev.base_page,
                        page_count: prev.page_count + center.page_count,
                        is_device: center.is_device,
                        used: false,
                    };
                    self.tree.insert(center);
                }
            }
        }

        if let Some(next_key) = self.tree.successor_key(center.base_page) {
            if let Some(next) = self.tree.get(next_key) {
                if !next.used
                    && next.is_device == center.is_device
                    && center.base_page + center.page_count == next.base_page
                {
                    self.tree.remove(center.base_page);
                    self.tree.remove(next.base_page);
                    center = Region {
                        base_page: center.base_page,
                        page_count: center.page_count + next.page_count,
                        is_device: center.is_device,
                        used: false,
                    };
                    self.tree.insert(center);
                }
            }
        }

        Ok(())
    }
}

#[inline(always)]
fn is_page_aligned(addr: usize) -> bool {
    (addr & (PAGE_SIZE - 1)) == 0
}

#[inline(always)]
fn bytes_to_pages(size_bytes: usize) -> usize {
    (size_bytes + PAGE_SIZE - 1) >> PAGE_BITS
}
