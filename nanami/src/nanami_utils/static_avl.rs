#[derive(Clone, Copy)]
struct AvlNode {
    used: bool,
    key: usize,
    value: usize,
    left: Option<usize>,
    right: Option<usize>,
    height: i16,
}

impl AvlNode {
    const EMPTY: Self = Self {
        used: false,
        key: 0,
        value: 0,
        left: None,
        right: None,
        height: 0,
    };
}

#[derive(Clone, Copy)]
pub struct StaticAvlTree<const N: usize> {
    root: Option<usize>,
    nodes: [AvlNode; N],
}

impl<const N: usize> StaticAvlTree<N> {
    pub const fn new() -> Self {
        Self {
            root: None,
            nodes: [AvlNode::EMPTY; N],
        }
    }

    pub fn insert(&mut self, key: usize, value: usize) -> Result<(), ()> {
        let new_root = self.insert_at(self.root, key, value)?;
        self.root = Some(new_root);
        Ok(())
    }

    pub fn find(&self, key: usize) -> Option<usize> {
        let mut cursor = self.root;
        while let Some(i) = cursor {
            let n = self.nodes[i];
            if key < n.key {
                cursor = n.left;
            } else if key > n.key {
                cursor = n.right;
            } else {
                return Some(n.value);
            }
        }
        None
    }

    fn insert_at(&mut self, current: Option<usize>, key: usize, value: usize) -> Result<usize, ()> {
        let Some(index) = current else {
            return self.allocate_node(key, value);
        };

        let node_key = self.nodes[index].key;
        if key < node_key {
            let left = self.insert_at(self.nodes[index].left, key, value)?;
            self.nodes[index].left = Some(left);
        } else if key > node_key {
            let right = self.insert_at(self.nodes[index].right, key, value)?;
            self.nodes[index].right = Some(right);
        } else {
            self.nodes[index].value = value;
            return Ok(index);
        }

        self.recompute_height(index);
        Ok(self.rebalance(index))
    }

    fn allocate_node(&mut self, key: usize, value: usize) -> Result<usize, ()> {
        let mut i = 0;
        while i < N {
            if !self.nodes[i].used {
                self.nodes[i] = AvlNode {
                    used: true,
                    key,
                    value,
                    left: None,
                    right: None,
                    height: 1,
                };
                return Ok(i);
            }
            i += 1;
        }
        Err(())
    }

    fn height(&self, index: Option<usize>) -> i16 {
        match index {
            Some(i) => self.nodes[i].height,
            None => 0,
        }
    }

    fn balance_factor(&self, index: usize) -> i16 {
        self.height(self.nodes[index].left) - self.height(self.nodes[index].right)
    }

    fn recompute_height(&mut self, index: usize) {
        let left_h = self.height(self.nodes[index].left);
        let right_h = self.height(self.nodes[index].right);
        self.nodes[index].height = if left_h > right_h {
            left_h + 1
        } else {
            right_h + 1
        };
    }

    fn rotate_left(&mut self, x: usize) -> usize {
        let y = self.nodes[x]
            .right
            .expect("rotate_left requires right child");
        let t2 = self.nodes[y].left;

        self.nodes[y].left = Some(x);
        self.nodes[x].right = t2;

        self.recompute_height(x);
        self.recompute_height(y);
        y
    }

    fn rotate_right(&mut self, y: usize) -> usize {
        let x = self.nodes[y]
            .left
            .expect("rotate_right requires left child");
        let t2 = self.nodes[x].right;

        self.nodes[x].right = Some(y);
        self.nodes[y].left = t2;

        self.recompute_height(y);
        self.recompute_height(x);
        x
    }

    fn rebalance(&mut self, index: usize) -> usize {
        let bf = self.balance_factor(index);

        if bf > 1 {
            let left = self.nodes[index]
                .left
                .expect("left-heavy without left child");
            if self.balance_factor(left) < 0 {
                let new_left = self.rotate_left(left);
                self.nodes[index].left = Some(new_left);
            }
            return self.rotate_right(index);
        }

        if bf < -1 {
            let right = self.nodes[index]
                .right
                .expect("right-heavy without right child");
            if self.balance_factor(right) > 0 {
                let new_right = self.rotate_right(right);
                self.nodes[index].right = Some(new_right);
            }
            return self.rotate_left(index);
        }

        index
    }
}
