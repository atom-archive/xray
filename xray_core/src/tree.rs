use std::clone::Clone;
use std::fmt;
use std::ops::{Add, AddAssign, Range};
use std::sync::Arc;

const MIN_CHILDREN: usize = 2;
const MAX_CHILDREN: usize = 4;

pub trait Item: Clone + Eq + fmt::Debug {
    type Summary: for<'a> AddAssign<&'a Self::Summary> + Default + Eq + Clone + fmt::Debug;

    fn summarize(&self) -> Self::Summary;
}

pub trait Dimension: for<'a> Add<&'a Self, Output=Self> + Ord + Clone + fmt::Debug {
    type Summary: Default + Eq + Clone + fmt::Debug;

    fn from_summary(summary: &Self::Summary) -> Self;

    fn default() -> Self {
        Self::from_summary(&Self::Summary::default())
    }
}

#[derive(Clone, Eq, PartialEq, Debug)]
pub struct Tree<T: Item>(Arc<Node<T>>);

#[derive(Clone, Eq, PartialEq, Debug)]
pub enum Node<T: Item> {
    Internal {
        rightmost_leaf: Option<Tree<T>>,
        summary: T::Summary,
        children: Vec<Tree<T>>,
        height: u16
    },
    Leaf {
        summary: T::Summary,
        value: T
    }
}

pub struct Iter<'a, T: 'a + Item> {
    tree: &'a Tree<T>,
    did_start: bool,
    stack: Vec<(&'a Tree<T>, usize)>,
}

#[derive(Debug)]
pub struct Cursor<'a, T: 'a + Item> {
    tree: &'a Tree<T>,
    did_seek: bool,
    stack: Vec<(&'a Tree<T>, usize)>,
    prev_leaf: Option<&'a Tree<T>>,
    summary: T::Summary
}

#[derive(Clone, Copy, Eq, PartialEq, Debug)]
pub enum SeekBias {
    Left,
    Right
}

impl<T: Item> Extend<T> for Tree<T> {
    fn extend<I: IntoIterator<Item=T>>(&mut self, items: I) {
        for item in items.into_iter() {
            self.push(item);
        }
    }
}

impl<'a, T: Item> Tree<T> {
    pub fn new() -> Self {
        Self::from_children(vec![])
    }

    fn from_children(children: Vec<Self>) -> Self {
        let summary = Self::summarize_children(&children);
        let rightmost_leaf = children.last().and_then(|last_child| {
            last_child.rightmost_leaf().cloned()
        });
        let height = children.get(0).map(|c| c.height()).unwrap_or(0) + 1;

        Tree(Arc::new(Node::Internal { rightmost_leaf, summary, children, height }))
    }

    fn summarize_children(children: &[Tree<T>]) -> T::Summary {
        let mut summary = T::Summary::default();
        for ref child in children {
            summary += child.summary();
        }
        summary
    }

    pub fn iter(&self) -> Iter<T> {
        Iter::new(self)
    }

    pub fn cursor(&self) -> Cursor<T> {
        Cursor::new(self)
    }

    pub fn len<D: Dimension<Summary=T::Summary>>(&self) -> D {
        D::from_summary(self.summary())
    }

    pub fn push(&mut self, item: T) {
        self.push_tree(Tree(Arc::new(Node::Leaf {
            summary: item.summarize(),
            value: item
        })))
    }

    pub fn push_tree(&mut self, other: Self) {
        if other.is_empty() { return }

        let self_height = self.height();
        let other_height = other.height();

        // Other is a taller tree, push its children one at a time
        if self_height < other_height {
            for other_child in other.children().iter().cloned() {
                self.push_tree(other_child);
            }
            return;
        }

        // Self is an internal node. Pushing other could cause the root to split.
        if let Some(split) = self.push_recursive(other) {
            *self = Self::from_children(vec![self.clone(), split])
        }
    }

    fn push_recursive(&mut self, other: Tree<T>) -> Option<Tree<T>> {
        *self.summary_mut() += other.summary();
        *self.rightmost_leaf_mut() = other.rightmost_leaf().cloned();

        let self_height = self.height();
        let other_height = other.height();

        if other_height == self_height  {
            self.append_children(other.children())
        } else if other_height == self_height - 1 && !other.underflowing() {
            self.append_children(&[other])
        } else {
            if let Some(split) = self.last_child_mut().push_recursive(other) {
                self.append_children(&[split])
            } else {
                None
            }
        }
    }

    fn append_children(&mut self, new_children: &[Tree<T>]) -> Option<Tree<T>> {
        match Arc::make_mut(&mut self.0) {
            &mut Node::Internal { ref mut children, ref mut summary, ref mut rightmost_leaf, .. } => {
                let child_count = children.len() + new_children.len();
                if child_count > MAX_CHILDREN {
                    let midpoint = (child_count + child_count % 2) / 2;
                    let (left_children, right_children): (Vec<Tree<T>>, Vec<Tree<T>>) = {
                        let mut all_children = children.iter().chain(new_children.iter()).cloned();
                        (all_children.by_ref().take(midpoint).collect(), all_children.collect())
                    };
                    *children = left_children;
                    *summary = Self::summarize_children(children);
                    *rightmost_leaf = children.last().unwrap().rightmost_leaf().cloned();
                    Some(Tree::from_children(right_children))
                } else {
                    children.extend(new_children.iter().cloned());
                    None
                }
            }
            &mut Node::Leaf { .. } => panic!("Tried to append children to a leaf node")
        }
    }

    #[allow(dead_code)]
    pub fn splice<D: Dimension<Summary=T::Summary>, I: IntoIterator<Item=T>>(&mut self, old_range: Range<&D>, new_items: I) {
        let mut result = Self::new();
        self.append_subsequence(&mut result, &D::default(), old_range.start);
        result.extend(new_items);
        self.append_subsequence(&mut result, old_range.end, &D::from_summary(self.summary()));
        *self = result;
    }

    fn append_subsequence<D: Dimension<Summary=T::Summary>>(&self, result: &mut Self, start: &D, end: &D) {
        self.append_subsequence_recursive(result, D::default(), start, end);
    }

    fn append_subsequence_recursive<D: Dimension<Summary=T::Summary>>(&self, result: &mut Self, node_start: D, start: &D, end: &D) {
        match self.0.as_ref() {
            &Node::Internal {ref summary, ref children, ..} => {
                let node_end = node_start.clone() + &D::from_summary(summary);
                if *start <= node_start && node_end <= *end {
                    result.push_tree(self.clone());
                } else if node_start < *end || *start < node_end {
                    let mut child_start = node_start.clone();
                    for ref child in children {
                        child.append_subsequence_recursive(result, child_start.clone(), start, end);
                        child_start = child_start + &D::from_summary(child.summary());
                    }
                }
            }
            &Node::Leaf {..} => {
                if *start <= node_start && node_start < *end {
                    result.push_tree(self.clone());
                }
            }
        }
    }

    fn rightmost_leaf(&self) -> Option<&Tree<T>> {
        match self.0.as_ref() {
            &Node::Internal { ref rightmost_leaf, .. } => rightmost_leaf.as_ref(),
            &Node::Leaf { .. } => Some(self)
        }
    }

    fn rightmost_leaf_mut(&mut self) -> &mut Option<Tree<T>> {
        match Arc::make_mut(&mut self.0) {
            &mut Node::Internal { ref mut rightmost_leaf, .. } => rightmost_leaf,
            _ => panic!("Requested a mutable reference to the rightmost leaf of a non-internal node"),
        }
    }

    fn summary(&self) -> &T::Summary {
        match self.0.as_ref() {
            &Node::Internal { ref summary, .. } => summary,
            &Node::Leaf { ref summary, .. } => summary,
        }
    }

    fn summary_mut(&mut self) -> &mut T::Summary {
        match Arc::make_mut(&mut self.0) {
            &mut Node::Internal { ref mut summary, .. } => summary,
            &mut Node::Leaf { ref mut summary, .. } => summary,
        }
    }

    fn children(&self) -> &[Tree<T>] {
        match self.0.as_ref() {
            &Node::Internal { ref children, .. } => children.as_slice(),
            &Node::Leaf { .. } => panic!("Requested children of a leaf node")
        }
    }

    fn last_child_mut(&mut self) -> &mut Tree<T> {
        match Arc::make_mut(&mut self.0) {
            &mut Node::Internal { ref mut children, .. } => children.last_mut().unwrap(),
            &mut Node::Leaf { .. } => panic!("Requested last child of a leaf node")
        }
    }

    fn value(&self) -> &T {
        match self.0.as_ref() {
            &Node::Internal { .. } => panic!("Requested value of an internal node"),
            &Node::Leaf { ref value, .. } => value,
        }
    }

    fn underflowing(&self) -> bool {
        match self.0.as_ref() {
            &Node::Internal { ref children, ..} => children.len() < MIN_CHILDREN,
            &Node::Leaf { .. } => false
        }
    }

    fn is_empty(&self) -> bool {
        match self.0.as_ref() {
            &Node::Internal { ref children, ..} => children.len() == 0,
            &Node::Leaf { .. } => false
        }
    }

    fn height(&self) -> u16 {
        match self.0.as_ref() {
            &Node::Internal { height, ..} => height,
            &Node::Leaf { .. } => 0
        }
    }
}

impl<'a, T: 'a + Item> Iter<'a, T> {
    fn new(tree: &'a Tree<T>) -> Self {
        Iter {
            tree,
            did_start: false,
            stack: Vec::with_capacity(tree.height() as usize)
        }
    }

    fn seek_to_first_item(&mut self, mut tree: &'a Tree<T>) -> Option<&'a T> {
        if tree.is_empty() {
            None
        } else {
            loop {
                match tree.0.as_ref() {
                    &Node::Internal { ref children, .. } => {
                        self.stack.push((tree, 0));
                        tree = &children[0];
                    }
                    &Node::Leaf { ref value, .. } => return Some(value),
                }
            }
        }
    }
}

impl<'a, T: 'a + Item> Iterator for Iter<'a, T> where Self: 'a {
    type Item = &'a T;

    fn next(&mut self) -> Option<Self::Item> {
        if self.did_start {
            while self.stack.len() > 0 {
                let (tree, index) = {
                    let &mut (tree, ref mut index) = self.stack.last_mut().unwrap();
                    *index += 1;
                    (tree, *index)
                };
                if let Some(child) = tree.children().get(index) {
                    return self.seek_to_first_item(child);
                } else {
                    self.stack.pop();
                }
            }
            None
        } else {
            self.did_start = true;
            self.seek_to_first_item(self.tree)
        }
    }
}

impl<'tree, T: 'tree + Item> Cursor<'tree, T> {
    fn new(tree: &'tree Tree<T>) -> Self {
        Self {
            tree,
            did_seek: false,
            stack: Vec::with_capacity(tree.height() as usize),
            prev_leaf: None,
            summary: T::Summary::default()
        }
    }

    fn reset(&mut self) {
        self.did_seek = false;
        self.stack.truncate(0);
        self.prev_leaf = None;
        self.summary = T::Summary::default();
    }

    pub fn start<D: Dimension<Summary=T::Summary>>(&self) -> D {
        D::from_summary(&self.summary)
    }

    pub fn item<'a>(&'a self) -> Option<&'tree T> {
        self.cur_leaf().map(|leaf| leaf.value())
    }

    pub fn prev_item<'a>(&'a self) -> Option<&'tree T> {
        self.prev_leaf.map(|leaf| leaf.value())
    }

    fn cur_leaf<'a>(&'a self) -> Option<&'tree Tree<T>> {
        assert!(self.did_seek, "Must seek before reading cursor position");
        self.stack.last().map(|&(subtree, index)| &subtree.children()[index])
    }

    pub fn next(&mut self) {
        assert!(self.did_seek, "Must seek before calling next");

        while self.stack.len() > 0 {
            let (prev_subtree, index) = {
                let &mut (prev_subtree, ref mut index) = self.stack.last_mut().unwrap();
                if prev_subtree.height() == 1 {
                    let prev_leaf = &prev_subtree.children()[*index];
                    self.prev_leaf = Some(prev_leaf);
                    self.summary += prev_leaf.summary();
                }
                *index += 1;
                (prev_subtree, *index)
            };
            if let Some(child) = prev_subtree.children().get(index) {
                self.seek_to_first_item(child);
                break;
            } else {
                self.stack.pop();
            }
        }
    }

    fn seek_to_first_item<'a>(&'a mut self, mut tree: &'tree Tree<T>) {
        self.did_seek = true;

        loop {
            match tree.0.as_ref() {
                &Node::Internal { ref children, ..} => {
                    self.stack.push((tree, 0));
                    tree = &children[0];
                }
                &Node::Leaf { .. } => {
                    break;
                }
            }
        }
    }

    #[allow(dead_code)]
    pub fn seek<D: Dimension<Summary=T::Summary>>(&mut self, pos: &D, bias: SeekBias) {
        self.seek_and_build_prefix(pos, bias, None);
    }
				
    pub fn build_prefix<D: Dimension<Summary=T::Summary>>(&mut self, end: &D, bias: SeekBias) -> Tree<T> {
        let mut prefix = Tree::new();
        self.seek_and_build_prefix(end, bias, Some(&mut prefix));
        prefix
    }

    fn seek_and_build_prefix<D: Dimension<Summary=T::Summary>>(&mut self, pos: &D, bias: SeekBias, mut prefix: Option<&mut Tree<T>>) {
        self.reset();
        self.did_seek = true;

        let mut cur_subtree = Some(self.tree);
        while let Some(subtree) = cur_subtree.take() {
            match subtree.0.as_ref() {
                &Node::Internal {ref rightmost_leaf, ref summary, ref children, ..} => {
                    let subtree_end = D::from_summary(&self.summary) + &D::from_summary(summary);
                    if *pos > subtree_end || (*pos == subtree_end && bias == SeekBias::Right) {
                        self.summary += summary;
                        self.prev_leaf = rightmost_leaf.as_ref();
                        prefix.as_mut().map(|prefix| prefix.push_tree(subtree.clone()));
                    } else {
                        for (index, child) in children.iter().enumerate() {
                            let child_end = D::from_summary(&self.summary) + &D::from_summary(child.summary());
                            if *pos > child_end || (*pos == child_end && bias == SeekBias::Right) {
                                self.summary += child.summary();
                                self.prev_leaf = child.rightmost_leaf();
                                prefix.as_mut().map(|prefix| prefix.push_tree(child.clone()));
                            } else {
                                self.stack.push((subtree, index));
                                cur_subtree = Some(child);
                                break;
                            }
                        }
                    }
                }
                &Node::Leaf {ref summary, ..} => {
                    // TODO? Can we push the child unconditionally?
                    let subtree_end = D::from_summary(&self.summary) + &D::from_summary(summary);
                    if *pos > subtree_end || (*pos == subtree_end && bias == SeekBias::Right) {
                        self.prev_leaf = Some(subtree);
                        self.summary += summary;
                        prefix.as_mut().map(|prefix| prefix.push_tree(subtree.clone()));
                    }
                }
            }
        }
    }

    pub fn build_suffix(&mut self) -> Tree<T> {
        assert!(self.did_seek, "Must seek before calling build_suffix");

        self.prev_leaf = self.tree.rightmost_leaf();
        self.summary = self.tree.summary().clone();

        if self.did_seek {
            let mut suffix = Tree::new();
            while let Some((subtree, index)) = self.stack.pop() {
                let start = if subtree.height() == 1 { index } else { index + 1 };
                for i in start..subtree.children().len() {
                    suffix.push_tree(subtree.children()[i].clone());
                }
            }
            suffix
        } else {
            self.did_seek = true;
            self.tree.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate rand;

    use super::*;

    #[derive(Default, Eq, PartialEq, Clone, Debug)]
    pub struct IntegersSummary {
        count: usize,
        sum: usize
    }

    #[derive(Ord, PartialOrd, Default, Eq, PartialEq, Clone, Debug)]
    struct Count(usize);

    #[derive(Ord, PartialOrd, Default, Eq, PartialEq, Clone, Debug)]
    struct Sum(usize);

    impl Item for u16 {
        type Summary = IntegersSummary;

        fn summarize(&self) -> Self::Summary {
            IntegersSummary {
                count: 1,
                sum: *self as usize
            }
        }
    }

    impl<'a> AddAssign<&'a Self> for IntegersSummary {
        fn add_assign(&mut self, other: &Self) {
            self.count += other.count;
            self.sum += other.sum;
        }
    }

    impl Dimension for Count {
        type Summary = IntegersSummary;

        fn from_summary(summary: &Self::Summary) -> Self {
            Count(summary.count)
        }
    }

    impl<'a> Add<&'a Self> for Count {
        type Output = Self;

        fn add(mut self, other: &Self) -> Self {
            self.0 += other.0;
            self
        }
    }

    impl Dimension for Sum {
        type Summary = IntegersSummary;

        fn from_summary(summary: &Self::Summary) -> Self {
            Sum(summary.sum)
        }
    }

    impl<'a> Add<&'a Self> for Sum {
        type Output = Self;

        fn add(mut self, other: &Self) -> Self {
            self.0 += other.0;
            self
        }
    }

    impl<T: super::Item> Tree<T> {
        fn items(&self) -> Vec<T> {
            self.iter().cloned().collect()
        }
    }

    #[test]
    fn test_extend_and_push() {
        let mut tree1 = Tree::new();
        tree1.extend(1..20);

        let mut tree2 = Tree::new();
        tree2.extend(1..50);

        tree1.push_tree(tree2);

        assert_eq!(
            tree1.items(),
            (1..20).chain(1..50).collect::<Vec<u16>>()
        );
    }

    #[test]
    fn splice() {
        let mut tree = Tree::new();
        tree.extend(0..10);
        tree.splice(&Count(2)..&Count(8), 20..23);
        assert_eq!(
            tree.items(),
            vec![0, 1, 20, 21, 22, 8, 9]
        );
    }

    #[test]
    fn random() {
        for seed in 0..100 {
            use self::rand::{Rng, SeedableRng, StdRng};
            let mut rng = StdRng::from_seed(&[seed]);

            let mut tree = Tree::<u16>::new();
            let count = rng.gen_range(0, 10);
            tree.extend(rng.gen_iter().take(count));

            for _i in 0..100 {
                let end = rng.gen_range(0, tree.len::<Count>().0 + 1);
                let start = rng.gen_range(0, end + 1);
                let count = rng.gen_range(0, 3);
                let new_items = rng.gen_iter().take(count).collect::<Vec<u16>>();
                let mut reference_items = tree.items();

                tree.splice(&Count(start)..&Count(end), new_items.clone());
                reference_items.splice(start..end, new_items);

                assert_eq!(tree.items(), reference_items);

                let mut cursor = tree.cursor();
                let suffix_start = rng.gen_range(0, tree.len::<Count>().0 + 1);
                let prefix_end = rng.gen_range(0, suffix_start + 1);

                let prefix_items = cursor.build_prefix(&Count(prefix_end), SeekBias::Right).items();
                assert_eq!(prefix_items, reference_items[0..prefix_end].to_vec());

                // Scan to the start of the suffix if we aren't already there
                if suffix_start > prefix_end {
                    for i in prefix_end..suffix_start {
                        assert_eq!(cursor.item(), reference_items.get(i));
                        assert_eq!(cursor.prev_item(), if i > 0 { reference_items.get(i - 1) } else { None });
                        assert_eq!(cursor.start::<Count>(), Count(i));
                        cursor.next();
                    }
                }

                let suffix_items = cursor.build_suffix().items();
                assert_eq!(suffix_items, reference_items[suffix_start..].to_vec());
            }
        }
    }

    #[test]
    fn cursor() {
        // Empty tree
        let tree = Tree::<u16>::new();
        let mut cursor = tree.cursor();
        assert_eq!(cursor.build_prefix(&Sum(0), SeekBias::Right), Tree::new());
        assert_eq!(cursor.item(), None);
        assert_eq!(cursor.prev_item(), None);
        assert_eq!(cursor.start::<Count>(), Count(0));
        assert_eq!(cursor.start::<Sum>(), Sum(0));

        // Single-element tree
        let mut tree = Tree::<u16>::new();
        tree.extend(vec![1]);
        let mut cursor = tree.cursor();
        assert_eq!(cursor.build_prefix(&Sum(0), SeekBias::Right), Tree::new());
        assert_eq!(cursor.item(), Some(&1));
        assert_eq!(cursor.prev_item(), None);
        assert_eq!(cursor.start::<Count>(), Count(0));
        assert_eq!(cursor.start::<Sum>(), Sum(0));

        cursor.next();
        assert_eq!(cursor.item(), None);
        assert_eq!(cursor.prev_item(), Some(&1));
        assert_eq!(cursor.start::<Count>(), Count(1));
        assert_eq!(cursor.start::<Sum>(), Sum(1));

        assert_eq!(cursor.build_prefix(&Sum(1), SeekBias::Right).items(), [1]);
        assert_eq!(cursor.item(), None);
        assert_eq!(cursor.prev_item(), Some(&1));
        assert_eq!(cursor.start::<Count>(), Count(1));
        assert_eq!(cursor.start::<Sum>(), Sum(1));

        cursor.seek(&Sum(0), SeekBias::Right);
        assert_eq!(cursor.build_suffix().items(), [1]);
        assert_eq!(cursor.item(), None);
        assert_eq!(cursor.prev_item(), Some(&1));
        assert_eq!(cursor.start::<Count>(), Count(1));
        assert_eq!(cursor.start::<Sum>(), Sum(1));

        // Multiple-element tree
        let mut tree = Tree::new();
        tree.extend(vec![1, 2, 3, 4, 5, 6]);
        let mut cursor = tree.cursor();

        assert_eq!(cursor.build_prefix(&Sum(4), SeekBias::Right).items(), [1, 2]);
        assert_eq!(cursor.item(), Some(&3));
        assert_eq!(cursor.prev_item(), Some(&2));
        assert_eq!(cursor.start::<Count>(), Count(2));
        assert_eq!(cursor.start::<Sum>(), Sum(3));

        cursor.next();
        assert_eq!(cursor.item(), Some(&4));
        assert_eq!(cursor.prev_item(), Some(&3));
        assert_eq!(cursor.start::<Count>(), Count(3));
        assert_eq!(cursor.start::<Sum>(), Sum(6));

        cursor.next();
        assert_eq!(cursor.item(), Some(&5));
        assert_eq!(cursor.prev_item(), Some(&4));
        assert_eq!(cursor.start::<Count>(), Count(4));
        assert_eq!(cursor.start::<Sum>(), Sum(10));

        cursor.next();
        assert_eq!(cursor.item(), Some(&6));
        assert_eq!(cursor.prev_item(), Some(&5));
        assert_eq!(cursor.start::<Count>(), Count(5));
        assert_eq!(cursor.start::<Sum>(), Sum(15));

        cursor.next();
        cursor.next();
        assert_eq!(cursor.item(), None);
        assert_eq!(cursor.prev_item(), Some(&6));
        assert_eq!(cursor.start::<Count>(), Count(6));
        assert_eq!(cursor.start::<Sum>(), Sum(21));

        assert_eq!(cursor.build_prefix(&tree.len::<Sum>(), SeekBias::Right).items(), tree.items());
        assert_eq!(cursor.item(), None);
        assert_eq!(cursor.prev_item(), Some(&6));
        assert_eq!(cursor.start::<Count>(), Count(6));
        assert_eq!(cursor.start::<Sum>(), Sum(21));

        // Suffixes are built from the cursor's current location to the end.
        cursor.seek(&Count(3), SeekBias::Right);
        assert_eq!(cursor.build_suffix().items(), [4, 5, 6]);
        assert_eq!(cursor.item(), None);
        assert_eq!(cursor.prev_item(), Some(&6));
        assert_eq!(cursor.start::<Count>(), Count(6));
        assert_eq!(cursor.start::<Sum>(), Sum(21));

        // Seeking can bias left or right
        cursor.seek(&Sum(1), SeekBias::Left);
        assert_eq!(cursor.item(), Some(&1));
        cursor.seek(&Sum(1), SeekBias::Right);
        assert_eq!(cursor.item(), Some(&2));
    }
}
