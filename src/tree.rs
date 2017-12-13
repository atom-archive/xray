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
    Empty {
        summary: T::Summary
    },
    Leaf {
        summary: T::Summary,
        value: T
    },
    Internal {
        rightmost_leaf: Tree<T>,
        summary: T::Summary,
        children: Vec<Tree<T>>,
        height: u16
    }
}

pub struct Iter<'a, T: 'a + Item> {
    tree: &'a Tree<T>,
    advance_on_next: bool,
    stack: Vec<(&'a Tree<T>, usize)>,
}

pub struct Cursor<'a, T: 'a + Item> {
    tree: &'a Tree<T>,
    did_seek: bool,
    advance_on_next: bool,
    stack: Vec<(&'a Tree<T>, usize)>,
    prev_leaf: Option<&'a Tree<T>>,
    summary: T::Summary
}

impl<T: Item> From<T> for Tree<T> {
    fn from(value: T) -> Self {
        Tree(Arc::new(Node::Leaf {
            summary: value.summarize(),
            value: value
        }))
    }
}

impl<T: Item> Extend<T> for Tree<T> {
    fn extend<I: IntoIterator<Item=T>>(&mut self, items: I) {
        for item in items.into_iter() {
            self.push(Self::from(item));
        }
    }
}

impl<'a, T: Item> Tree<T> {
    pub fn new() -> Self {
        Tree(Arc::new(Node::Empty {
            summary: T::Summary::default()
        }))
    }

    fn from_children(children: Vec<Self>) -> Self {
        let summary = Self::summarize_children(&children);
        let rightmost_leaf = children.last().unwrap().rightmost_leaf().unwrap().clone();
        let height = children[0].height() + 1;
        Tree(Arc::new(Node::Internal { rightmost_leaf, summary, children, height }))
    }

    fn summarize_children(children: &[Tree<T>]) -> T::Summary {
        let mut iter = children.iter();
        let mut summary = iter.next().unwrap().summary().clone();
        for ref child in iter {
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

    // This should only be called on the root.
    pub fn push<S: Into<Tree<T>>>(&mut self, other: S) {
        let other = other.into();

        if other.is_empty() {
            return;
        }

        if self.is_empty() {
            *self = other;
            return;
        }

        let self_height = self.height();
        let other_height = other.height();

        // Other is a taller tree, push its children one at a time
        if self_height < other_height {
            for other_child in other.children().iter().cloned() {
                self.push(other_child);
            }
            return;
        }

        // At this point, we know that other isn't taller than self and isn't empty.
        // Therefore, we're pushing a leaf onto a leaf, so we reassign root to an internal node.
        if self_height == 0 {
            *self = Self::from_children(vec![self.clone(), other]);
            return;
        }

        // Self is an internal node. Pushing other could cause the root to split.
        if let Some(split) = self.push_recursive(other) {
            *self = Self::from_children(vec![self.clone(), split])
        }
    }

    fn push_recursive(&mut self, other: Tree<T>) -> Option<Tree<T>> {
        *self.summary_mut() += other.summary();
        *self.rightmost_leaf_mut() = other.rightmost_leaf().unwrap().clone();

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
            &mut Node::Internal { ref mut rightmost_leaf, ref mut summary, ref mut children, .. } => {
                let child_count = children.len() + new_children.len();
                if child_count > MAX_CHILDREN {
                    let midpoint = (child_count + child_count % 2) / 2;
                    let (left_children, right_children): (Vec<Tree<T>>, Vec<Tree<T>>) = {
                        let mut all_children = children.iter().chain(new_children.iter()).cloned();
                        (all_children.by_ref().take(midpoint).collect(), all_children.collect())
                    };
                    *rightmost_leaf = left_children.last().unwrap().rightmost_leaf().unwrap().clone();
                    *summary = Self::summarize_children(&left_children);
                    *children = left_children;
                    Some(Tree::from_children(right_children))
                } else {
                    children.extend(new_children.iter().cloned());
                    None
                }
            },
            _ => panic!("Tried to append children to a non-internal node")
        }
    }

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
            &Node::Empty {..} => (),
            &Node::Leaf {..} => {
                if *start <= node_start && node_start < *end {
                    result.push(self.clone());
                }
            }
            &Node::Internal {ref summary, ref children, ..} => {
                let node_end = node_start.clone() + &D::from_summary(summary);
                if *start <= node_start && node_end <= *end {
                    result.push(self.clone());
                } else if node_start < *end || *start < node_end {
                    let mut child_start = node_start.clone();
                    for ref child in children {
                        child.append_subsequence_recursive(result, child_start.clone(), start, end);
                        child_start = child_start + &D::from_summary(child.summary());
                    }
                }
            }
        }
    }

    fn rightmost_leaf(&self) -> Option<&Tree<T>> {
        match self.0.as_ref() {
            &Node::Empty { .. } => None,
            &Node::Leaf { .. } => Some(self),
            &Node::Internal { ref rightmost_leaf, .. } => Some(rightmost_leaf)
        }
    }

    fn rightmost_leaf_mut(&mut self) -> &mut Tree<T> {
        match Arc::make_mut(&mut self.0) {
            &mut Node::Internal { ref mut rightmost_leaf, .. } => rightmost_leaf,
            _ => panic!("Requested a mutable reference to the rightmost leaf of a non-internal node"),
        }
    }

    fn summary(&self) -> &T::Summary {
        match self.0.as_ref() {
            &Node::Empty { ref summary } => summary,
            &Node::Leaf { ref summary, .. } => summary,
            &Node::Internal { ref summary, .. } => summary,
        }
    }

    fn summary_mut(&mut self) -> &mut T::Summary {
        match Arc::make_mut(&mut self.0) {
            &mut Node::Empty { .. } => panic!("Requested a summary of an empty node"),
            &mut Node::Leaf { ref mut summary, .. } => summary,
            &mut Node::Internal { ref mut summary, .. } => summary,
        }
    }

    fn children(&self) -> &[Tree<T>] {
        match self.0.as_ref() {
            &Node::Internal { ref children, .. } => children.as_slice(),
            _ => panic!("Requested children of a non-internal node")
        }
    }

    fn last_child_mut(&mut self) -> &mut Tree<T> {
        match Arc::make_mut(&mut self.0) {
            &mut Node::Internal { ref mut children, .. } => children.last_mut().unwrap(),
            _ => panic!("Requested last child of a non-internal node")
        }
    }

    fn value(&self) -> &T {
        match self.0.as_ref() {
            &Node::Leaf { ref value, .. } => value,
            _ => panic!("Requested value of a non-leaf node")
        }
    }

    fn underflowing(&self) -> bool {
        match self.0.as_ref() {
            &Node::Internal { ref children, ..} => children.len() < MIN_CHILDREN,
            _ => false
        }
    }

    fn is_empty(&self) -> bool {
        match self.0.as_ref() {
            &Node::Empty { .. } => true,
            _ => false
        }
    }

    fn height(&self) -> u16 {
        match self.0.as_ref() {
            &Node::Internal { height, ..} => height,
            _ => 0
        }
    }
}

impl<'a, T: 'a + Item> Iter<'a, T> {
    fn new(tree: &'a Tree<T>) -> Self {
        Iter {
            tree,
            advance_on_next: false,
            stack: Vec::with_capacity(tree.height() as usize)
        }
    }

    fn descend_to_first_item(&mut self, mut tree: &'a Tree<T>) -> Option<&'a T> {
        loop {
            match tree.0.as_ref() {
                &Node::Empty { .. } => return None,
                &Node::Leaf { ref value, .. } => return Some(value),
                &Node::Internal { ref children, .. } => {
                    self.stack.push((tree, 0));
                    tree = &children[0];
                }
            }
        }
    }
}

impl<'a, T: 'a + Item> Iterator for Iter<'a, T> where Self: 'a {
    type Item = &'a T;

    fn next(&mut self) -> Option<Self::Item> {
        if self.advance_on_next {
            while self.stack.len() > 0 {
                let (tree, index) = {
                    let &mut (tree, ref mut index) = self.stack.last_mut().unwrap();
                    *index += 1;
                    (tree, *index)
                };
                if let Some(child) = tree.children().get(index) {
                    return self.descend_to_first_item(child);
                } else {
                    self.stack.pop();
                }
            }
            None
        } else {
            self.advance_on_next = true;
            self.descend_to_first_item(self.tree)
        }
    }
}

impl<'tree, T: 'tree + Item> Cursor<'tree, T> {
    fn new(tree: &'tree Tree<T>) -> Self {
        Self {
            tree,
            did_seek: false,
            advance_on_next: false,
            stack: Vec::with_capacity(tree.height() as usize),
            prev_leaf: None,
            summary: T::Summary::default()
        }
    }

    fn reset(&mut self) {
        self.did_seek = false;
        self.advance_on_next = false;
        self.stack.truncate(0);
        self.prev_leaf = None;
        self.summary = T::Summary::default();
    }

    pub fn next(&mut self) -> (Option<&'tree T>, Option<&'tree T>, &T::Summary) {
        if self.did_seek {
            match self.tree.0.as_ref() {
                &Node::Internal { .. } => {
                    if self.advance_on_next {
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
                                return self.descend_to_first_item(child);
                            } else {
                                self.stack.pop();
                            }
                        }
                        (self.prev_element(), None, &self.summary)
                    } else {
                        self.advance_on_next = true;
                        let cur_element = self.stack.last().map(|&(subtree, index)| {
                            subtree.children()[index].value()
                        });
                        (self.prev_element(), cur_element, &self.summary)
                    }
                },
                &Node::Leaf { ref value, ref summary, .. } => {
                    if self.advance_on_next {
                        (Some(value), None, summary)
                    } else {
                        self.advance_on_next = true;
                        (None, Some(value), &self.summary)
                    }
                },
                &Node::Empty { .. } => (None, None, &self.summary)
            }
        } else {
            self.advance_on_next = true;
            self.descend_to_first_item(self.tree)
        }
    }

    fn descend_to_first_item<'a>(&'a mut self, mut tree: &'tree Tree<T>) -> (Option<&'tree T>, Option<&'tree T>, &'a T::Summary) {
        self.did_seek = true;

        loop {
            match tree.0.as_ref() {
                &Node::Empty { .. } => {
                    return (None, None, &self.summary);
                }
                &Node::Leaf {ref value, ..} => {
                    return (self.prev_element(), Some(value), &self.summary);
                },
                &Node::Internal { ref children, ..} => {
                    self.stack.push((tree, 0));
                    tree = &children[0];
                }
            }
        }
    }

    pub fn seek<D: Dimension<Summary=T::Summary>>(&mut self, pos: &D) {
        self.seek_internal(pos, None);
    }

    pub fn build_prefix<D: Dimension<Summary=T::Summary>>(&mut self, end: &D) -> Tree<T> {
        let mut prefix = Tree::new();
        self.seek_internal(end, Some(&mut prefix));
        prefix
    }

    fn seek_internal<D: Dimension<Summary=T::Summary>>(&mut self, pos: &D, mut prefix: Option<&mut Tree<T>>) {
        self.reset();
        self.did_seek = true;

        let mut subtree = self.tree;
        loop {
            match subtree.0.as_ref() {
                &Node::Internal {ref rightmost_leaf, ref summary, ref children, ..} => {
                    let subtree_end = D::from_summary(&self.summary) + &D::from_summary(summary);
                    if *pos >= subtree_end {
                        self.prev_leaf = Some(rightmost_leaf);
                        self.summary += summary;
                        prefix.as_mut().map(|prefix| prefix.push(subtree.clone()));
                        return;
                    } else {
                        for (index, child) in children.iter().enumerate() {
                            let child_end = D::from_summary(&self.summary) + &D::from_summary(child.summary());
                            if *pos >= child_end {
                                self.prev_leaf = child.rightmost_leaf();
                                self.summary += child.summary();
                                prefix.as_mut().map(|prefix| prefix.push(child.clone()));
                            } else {
                                self.stack.push((subtree, index));
                                subtree = child;
                                break;
                            }
                        }
                    }
                }
                &Node::Leaf {ref summary, ..} => {
                    let subtree_end = D::from_summary(&self.summary) + &D::from_summary(summary);
                    if *pos >= subtree_end {
                        self.advance_on_next = true;
                        self.prev_leaf = Some(subtree);
                        self.summary += summary;
                        prefix.as_mut().map(|prefix| prefix.push(subtree.clone()));
                    }
                    return;
                }
                &Node::Empty { .. } => return
            }
        }
    }

    pub fn build_suffix(&mut self) -> Tree<T> {
        if !self.did_seek {
            return self.tree.clone()
        }

        let suffix = match self.tree.0.as_ref() {
            &Node::Internal { .. } => {
                let mut suffix = Tree::new();
                while let Some((subtree, index)) = self.stack.pop() {
                    let start = if subtree.height() == 1 { index } else { index + 1 };
                    for i in start..subtree.children().len() {
                        suffix.push(subtree.children()[i].clone());
                    }
                }
                suffix
            },
            &Node::Leaf { .. } => {
                if self.advance_on_next {
                    Tree::new()
                } else {
                    self.tree.clone()
                }
            },
            &Node::Empty { ..} => Tree::new()
        };

        self.prev_leaf = self.tree.rightmost_leaf();
        self.summary = self.tree.summary().clone();
        suffix
    }

    fn prev_element(&self) -> Option<&'tree T> {
        self.prev_leaf.map(|leaf| leaf.value())
    }
}

#[cfg(test)]
mod tests {
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
        tree1.extend((1..20));

        let mut tree2 = Tree::new();
        tree2.extend((1..50));

        tree1.push(tree2);

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
    fn random_splices_and_cursor_operations() {
        for seed in 0..100 {
            use rand::{Rng, SeedableRng, StdRng};
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

                let prefix_items = cursor.build_prefix(&Count(prefix_end)).items();
                assert_eq!(prefix_items, reference_items[0..prefix_end].to_vec());

                // Scan to the start of the suffix if we aren't already there
                if suffix_start > prefix_end {
                    for i in prefix_end..suffix_start + 1 {
                        let (prev_item, cur_item, summary) = cursor.next();
                        assert_eq!(cur_item, reference_items.get(i));
                        assert_eq!(prev_item, if i > 0 { reference_items.get(i - 1) } else { None });
                        assert_eq!(summary.count, i);
                    }
                }

                let suffix_items = cursor.build_suffix().items();
                assert_eq!(suffix_items, reference_items[suffix_start..].to_vec());
            }
        }
    }

    #[test]
    fn cursor_operations() {
        // Empty tree
        let tree = Tree::<u16>::new();
        let mut cursor = tree.cursor();
        assert_eq!(cursor.build_prefix(&Sum(0)), Tree::new());
        assert_eq!(cursor.next(), (None, None, &IntegersSummary::default()));

        // Single-element tree
        let mut tree = Tree::<u16>::new();
        tree.extend(vec![1]);
        let mut cursor = tree.cursor();
        assert_eq!(cursor.build_prefix(&Sum(0)), Tree::new());
        assert_eq!(cursor.next(), (None, Some(&1), &IntegersSummary::default()));
        assert_eq!(cursor.next(), (Some(&1), None, &IntegersSummary {count: 1, sum: 1}));
        assert_eq!(cursor.build_prefix(&Sum(1)).items(), [1]);
        assert_eq!(cursor.next(), (Some(&1), None, &IntegersSummary {count: 1, sum: 1}));
        cursor.seek(&Sum(0));
        assert_eq!(cursor.build_suffix().items(), [1]);

        // Multiple-element tree
        let mut tree = Tree::new();
        tree.extend(vec![1, 2, 3, 4, 5, 6]);
        let mut cursor = tree.cursor();

        // Calling next without building a prefix yields the first element
        assert_eq!(cursor.next(), (None, Some(&1), &IntegersSummary {count: 0, sum: 0}));

        // Calling next after building a prefix yields the element after the last prefix
        assert_eq!(cursor.build_prefix(&Sum(4)).items(), [1, 2]);
        assert_eq!(cursor.next(), (Some(&2), Some(&3), &IntegersSummary {count: 2, sum: 3}));
        assert_eq!(cursor.next(), (Some(&3), Some(&4), &IntegersSummary {count: 3, sum: 6}));
        assert_eq!(cursor.next(), (Some(&4), Some(&5), &IntegersSummary {count: 4, sum: 10}));
        assert_eq!(cursor.next(), (Some(&5), Some(&6), &IntegersSummary {count: 5, sum: 15}));
        assert_eq!(cursor.next(), (Some(&6), None, &IntegersSummary {count: 6, sum: 21}));
        assert_eq!(cursor.next(), (Some(&6), None, &IntegersSummary {count: 6, sum: 21}));
        assert_eq!(cursor.build_prefix(&tree.len::<Sum>()).items(), tree.items());
        assert_eq!(cursor.next(), (Some(&6), None, &IntegersSummary {count: 6, sum: 21}));
        assert_eq!(cursor.next(), (Some(&6), None, &IntegersSummary {count: 6, sum: 21}));

        // Suffixes are built from the cursor's current element to the end
        cursor.seek(&Count(3));
        assert_eq!(cursor.build_suffix().items(), [4, 5, 6]);
        assert_eq!(cursor.next(), (Some(&6), None, &IntegersSummary {count: 6, sum: 21}));
        assert_eq!(cursor.next(), (Some(&6), None, &IntegersSummary {count: 6, sum: 21}));
        assert_eq!(cursor.build_suffix().items(), []);

        // Calling build suffix without seeking yields the entire tree
        let mut cursor = tree.cursor();
        assert_eq!(cursor.build_suffix().items(), tree.items());
    }
}
