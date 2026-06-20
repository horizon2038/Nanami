use alloc::boxed::Box;

struct AvlNode {
    key: usize,
    value: usize,
    left: Option<Box<AvlNode>>,
    right: Option<Box<AvlNode>>,
    height: i16,
}

impl AvlNode {
    fn new(key: usize, value: usize) -> Box<Self> {
        Box::new(Self {
            key,
            value,
            left: None,
            right: None,
            height: 1,
        })
    }
}

pub struct AvlTree {
    root: Option<Box<AvlNode>>,
}

impl AvlTree {
    pub const fn new() -> Self {
        Self { root: None }
    }

    pub fn insert(&mut self, key: usize, value: usize) -> Result<(), ()> {
        self.root = Some(insert_node(self.root.take(), key, value));
        Ok(())
    }

    pub fn find(&self, key: usize) -> Option<usize> {
        let mut cursor = self.root.as_ref();
        while let Some(node) = cursor {
            if key < node.key {
                cursor = node.left.as_ref();
            } else if key > node.key {
                cursor = node.right.as_ref();
            } else {
                return Some(node.value);
            }
        }
        None
    }
}

fn insert_node(node: Option<Box<AvlNode>>, key: usize, value: usize) -> Box<AvlNode> {
    let Some(mut node) = node else {
        return AvlNode::new(key, value);
    };

    if key < node.key {
        node.left = Some(insert_node(node.left.take(), key, value));
    } else if key > node.key {
        node.right = Some(insert_node(node.right.take(), key, value));
    } else {
        node.value = value;
        return node;
    }

    recompute_height(&mut node);
    rebalance(node)
}

fn height(node: &Option<Box<AvlNode>>) -> i16 {
    node.as_ref().map(|n| n.height).unwrap_or(0)
}

fn recompute_height(node: &mut Box<AvlNode>) {
    let left_h = height(&node.left);
    let right_h = height(&node.right);
    node.height = if left_h > right_h {
        left_h + 1
    } else {
        right_h + 1
    };
}

fn balance_factor(node: &AvlNode) -> i16 {
    height(&node.left) - height(&node.right)
}

fn rotate_left(mut x: Box<AvlNode>) -> Box<AvlNode> {
    let Some(mut y) = x.right.take() else {
        return x;
    };
    let t2 = y.left.take();

    x.right = t2;
    recompute_height(&mut x);

    y.left = Some(x);
    recompute_height(&mut y);
    y
}

fn rotate_right(mut y: Box<AvlNode>) -> Box<AvlNode> {
    let Some(mut x) = y.left.take() else {
        return y;
    };
    let t2 = x.right.take();

    y.left = t2;
    recompute_height(&mut y);

    x.right = Some(y);
    recompute_height(&mut x);
    x
}

fn rebalance(mut node: Box<AvlNode>) -> Box<AvlNode> {
    let bf = balance_factor(&node);

    if bf > 1 {
        if node.left.as_ref().map(|n| balance_factor(n)).unwrap_or(0) < 0 {
            node.left = node.left.take().map(rotate_left);
        }
        return rotate_right(node);
    }

    if bf < -1 {
        if node.right.as_ref().map(|n| balance_factor(n)).unwrap_or(0) > 0 {
            node.right = node.right.take().map(rotate_right);
        }
        return rotate_left(node);
    }

    node
}
