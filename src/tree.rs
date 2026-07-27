//! Literal AVL-tree translation of `tree.c`.

use std::cmp::Ordering;

// [spec:samurai:def:tree.treenode]
pub struct TreeNode<V> {
    pub key: String,
    pub value: V,
    pub child: [Option<Box<TreeNode<V>>>; 2],
    pub height: i32,
}

// [spec:samurai:def:tree.deltree-fn]
// [spec:samurai:sem:tree.deltree-fn]
pub fn deltree<V>(
    node: Option<Box<TreeNode<V>>>,
    delkey: Option<fn(String)>,
    delval: Option<fn(V)>,
) {
    if let Some(node) = node {
        let TreeNode {
            key,
            value,
            child: [left, right],
            ..
        } = *node;
        if let Some(delkey) = delkey {
            delkey(key);
        }
        if let Some(delval) = delval {
            delval(value);
        }
        deltree(left, delkey, delval);
        deltree(right, delkey, delval);
    }
}

// [spec:samurai:def:tree.height-fn]
// [spec:samurai:sem:tree.height-fn]
fn height<V>(node: &Option<Box<TreeNode<V>>>) -> i32 {
    node.as_ref().map_or(0, |node| node.height)
}

fn update_height<V>(node: &mut Box<TreeNode<V>>) {
    node.height = 1 + height(&node.child[0]).max(height(&node.child[1]));
}

fn rotate_left<V>(root: &mut Option<Box<TreeNode<V>>>) {
    let mut old_root = root.take().expect("rotation root");
    let mut pivot = old_root.child[1].take().expect("right child");
    old_root.child[1] = pivot.child[0].take();
    update_height(&mut old_root);
    pivot.child[0] = Some(old_root);
    update_height(&mut pivot);
    *root = Some(pivot);
}

fn rotate_right<V>(root: &mut Option<Box<TreeNode<V>>>) {
    let mut old_root = root.take().expect("rotation root");
    let mut pivot = old_root.child[0].take().expect("left child");
    old_root.child[0] = pivot.child[1].take();
    update_height(&mut old_root);
    pivot.child[1] = Some(old_root);
    update_height(&mut pivot);
    *root = Some(pivot);
}

// [spec:samurai:def:tree.rot-fn]
// [spec:samurai:sem:tree.rot-fn]
fn rot<V>(root: &mut Option<Box<TreeNode<V>>>, dir: usize) -> i32 {
    let old_height = height(root);
    let child = root.as_ref().expect("rotation root").child[dir]
        .as_ref()
        .expect("deep child");
    if height(&child.child[1 - dir]) > height(&child.child[dir]) {
        if dir == 0 {
            rotate_left(&mut root.as_mut().expect("root").child[0]);
        } else {
            rotate_right(&mut root.as_mut().expect("root").child[1]);
        }
    }
    if dir == 0 {
        rotate_right(root);
    } else {
        rotate_left(root);
    }
    height(root) - old_height
}

// [spec:samurai:def:tree.balance-fn]
// [spec:samurai:sem:tree.balance-fn]
fn balance<V>(root: &mut Option<Box<TreeNode<V>>>) -> i32 {
    let node = root.as_ref().expect("balance root");
    let left = height(&node.child[0]);
    let right = height(&node.child[1]);
    if (left - right).abs() <= 1 {
        let old = node.height;
        update_height(root.as_mut().expect("balance root"));
        height(root) - old
    } else {
        rot(root, usize::from(left < right))
    }
}

// [spec:samurai:def:tree.treefind-fn]
// [spec:samurai:sem:tree.treefind-fn]
pub fn treefind<'a, V>(mut node: Option<&'a TreeNode<V>>, key: &str) -> Option<&'a TreeNode<V>> {
    while let Some(current) = node {
        match key.cmp(&current.key) {
            Ordering::Equal => return Some(current),
            Ordering::Less => node = current.child[0].as_deref(),
            Ordering::Greater => node = current.child[1].as_deref(),
        }
    }
    None
}

fn insert<V>(root: &mut Option<Box<TreeNode<V>>>, key: String, value: V) -> Option<V> {
    match root {
        None => {
            *root = Some(Box::new(TreeNode {
                key,
                value,
                child: [None, None],
                height: 1,
            }));
            None
        }
        Some(node) => {
            let result = match key.cmp(&node.key) {
                Ordering::Equal => Some(std::mem::replace(&mut node.value, value)),
                Ordering::Less => insert(&mut node.child[0], key, value),
                Ordering::Greater => insert(&mut node.child[1], key, value),
            };
            if result.is_none() {
                balance(root);
            }
            result
        }
    }
}

// [spec:samurai:def:tree.treeinsert-fn]
// [spec:samurai:sem:tree.treeinsert-fn]
pub fn treeinsert<V>(root: &mut Option<Box<TreeNode<V>>>, key: String, value: V) -> Option<V> {
    insert(root, key, value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn balances_insertions_and_replaces_values() {
        let mut root = None;
        for key in ["c", "b", "a", "d", "e"] {
            assert_eq!(treeinsert(&mut root, key.into(), key.len()), None);
        }
        assert_eq!(
            treefind(root.as_deref(), "a").map(|node| node.value),
            Some(1)
        );
        assert_eq!(treeinsert(&mut root, "a".into(), 9), Some(1));
        assert_eq!(
            treefind(root.as_deref(), "a").map(|node| node.value),
            Some(9)
        );
    }
}
