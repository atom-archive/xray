use std::clone::Clone;
use std::fmt;
use std::ops::Range;
use std::sync::Arc;

const MIN_CHILDREN: usize = 2;
const MAX_CHILDREN: usize = 4;

pub trait Item: Clone + Eq + fmt::Debug {
    type Summary: Summary;

    fn summarize(&self) -> Self::Summary;
}

pub trait Summary: Clone + Eq + fmt::Debug {
    fn accumulate(&mut self, other: &Self);
}

pub trait Dimension: Default + Ord + Clone + fmt::Debug {
    type Summary: Summary;

    fn from_summary(summary: &Self::Summary) -> Self;

    fn accumulate(&self, other: &Self) -> Self;
}

#[derive(Clone, Eq, PartialEq, Debug)]
pub struct Tree<T: Item>(Arc<Node<T>>);

#[derive(Clone, Eq, PartialEq, Debug)]
pub enum Node<T: Item> {
    Empty,
    Leaf {
        summary: T::Summary,
        value: T
    },
    Internal {
        summary: T::Summary,
        children: Vec<Tree<T>>,
        height: u16
    }
}

pub struct Iter<'a, T: 'a + Item> {
    tree: &'a Tree<T>,
    started: bool,
    stack: Vec<(&'a Tree<T>, usize)>,
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
        Tree(Arc::new(Node::Empty))
    }

    fn from_children(children: Vec<Self>) -> Self {
        let summary = Self::summarize_children(&children);
        let height = children[0].height() + 1;
        Tree(Arc::new(Node::Internal { summary, children, height }))
    }

    fn summarize_children(children: &[Tree<T>]) -> T::Summary {
        let mut iter = children.iter();
        let mut summary = iter.next().unwrap().summary().clone();
        for ref child in iter {
            summary.accumulate(child.summary());
        }
        summary
    }

    fn iter(&self) -> Iter<T> {
        Iter::new(self)
    }

    fn len<D: Dimension<Summary=T::Summary>>(&self) -> D {
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
        if self_height == 1 {
            *self = Self::from_children(vec![self.clone(), other]);
            return;
        }

        // Self is an internal node. Pushing other could cause the root to split.
        if let Some(split) = self.push_recursive(other) {
            *self = Self::from_children(vec![self.clone(), split])
        }
    }

    fn push_recursive(&mut self, other: Tree<T>) -> Option<Tree<T>> {
        self.summary_mut().accumulate(other.summary());

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
            &mut Node::Internal { ref mut summary, ref mut children, .. } => {
                let child_count = children.len() + new_children.len();
                if child_count > MAX_CHILDREN {
                    let midpoint = (child_count + child_count % 2) / 2;
                    let (left_children, right_children): (Vec<Tree<T>>, Vec<Tree<T>>) = {
                        let mut all_children = children.iter().chain(new_children.iter()).cloned();
                        (all_children.by_ref().take(midpoint).collect(), all_children.collect())
                    };
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
        self.append_subsequence_recursive(result, &D::default(), start, end);
    }

    fn append_subsequence_recursive<D: Dimension<Summary=T::Summary>>(&self, result: &mut Self, node_start: &D, start: &D, end: &D) {
        match self.0.as_ref() {
            &Node::Empty => (),
            &Node::Leaf {..} => {
                if *start <= *node_start && *node_start < *end {
                    result.push(self.clone());
                }
            }
            &Node::Internal {ref summary, ref children, ..} => {
                let node_end = node_start.accumulate(&D::from_summary(summary));
                if *start <= *node_start && node_end <= *end {
                    result.push(self.clone());
                } else if *node_start < *end || *start < node_end {
                    let mut child_start = node_start.clone();
                    for ref child in children {
                        child.append_subsequence_recursive(result, &child_start, start, end);
                        child_start = child_start.accumulate(&D::from_summary(child.summary()));
                    }
                }
            }

        }
    }

    pub fn get<D: Dimension<Summary=T::Summary>>(&self, target: D) -> Option<&T> {
        self.get_internal(D::default(), target)
    }

    fn get_internal<D: Dimension<Summary=T::Summary>>(&self, mut current_pos: D, target: D) -> Option<&T> {
        match self.0.as_ref() {
            &Node::Empty => None,
            &Node::Leaf {ref value, ..} => Some(value),
            &Node::Internal {ref children, ..} => {
                for ref child in children {
                    let next_pos = current_pos.accumulate(&D::from_summary(&child.summary()));
                    if next_pos > target {
                        return child.get_internal(current_pos, target);
                    } else {
                        current_pos = next_pos;
                    }
                }
                None
            }
        }
    }

    fn summary(&self) -> &T::Summary {
        match self.0.as_ref() {
            &Node::Empty => panic!("Requested a summary of an empty node"),
            &Node::Leaf { ref summary, .. } => summary,
            &Node::Internal { ref summary, .. } => summary,
        }
    }

    fn summary_mut(&mut self) -> &mut T::Summary {
        match Arc::make_mut(&mut self.0) {
            &mut Node::Empty => panic!("Requested a summary of an empty node"),
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
            &Node::Empty => true,
            _ => false
        }
    }

    fn height(&self) -> u16 {
        match self.0.as_ref() {
            &Node::Empty => 1,
            &Node::Leaf { .. } => 1,
            &Node::Internal { height, ..} => height
        }
    }
}

impl<'a, T: 'a + Item> Iterator for Iter<'a, T> where Self: 'a {
    type Item = &'a T;

    fn next(&mut self) -> Option<Self::Item> {
        if self.started {
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
            self.started = true;
            self.descend_to_first_item(self.tree)
        }
    }
}

impl<'a, T: 'a + Item> Iter<'a, T> {
    fn new(tree: &'a Tree<T>) -> Self {
        Iter {
            tree,
            started: false,
            stack: Vec::with_capacity((tree.height() - 1) as usize)
        }
    }

    fn descend_to_first_item(&mut self, mut tree: &'a Tree<T>) -> Option<&'a T> {
        loop {
            match tree.0.as_ref() {
                &Node::Empty => return None,
                &Node::Leaf {ref value, ..} => return Some(value),
                &Node::Internal { ref children, ..} => {
                    self.stack.push((tree, 0));
                    tree = &children[0];
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    impl Item for u16 {
        type Summary = usize;

        fn summarize(&self) -> usize {
            1
        }
    }

    impl Summary for usize {
        fn accumulate(&mut self, other: &Self) {
            *self += *other;
        }
    }

    impl Dimension for usize {
        type Summary = usize;

        fn from_summary(summary: &Self::Summary) -> Self {
            *summary
        }

        fn accumulate(&self, other: &Self) -> Self {
            *self + *other
        }
    }

    impl<T: super::Item> Tree<T> {
        fn items(&self) -> Vec<T> {
            self.iter().cloned().collect()
        }
    }

    #[test]
    fn extend_and_push() {
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
        tree.splice(&2..&8, 20..23);
        assert_eq!(
            tree.items(),
            vec![0, 1, 20, 21, 22, 8, 9]
        );
    }

    #[test]
    fn random_splice() {
        use rand::{self, Rng};
        let mut rng = rand::thread_rng();

        let mut tree = Tree::<u16>::new();
        let count = rng.gen_range(0, 100);
        tree.extend(rng.gen_iter().take(count));

        for i in 0..100 {
            let end = rng.gen_range(0, tree.len::<usize>() + 1);
            let start = rng.gen_range(0, end + 1);
            let count = rng.gen_range(0, 100);
            let new_items = rng.gen_iter().take(count).collect::<Vec<u16>>();
            let mut original_tree_items = tree.items();

            tree.splice(&start..&end, new_items.clone());
            original_tree_items.splice(start..end, new_items);

            assert_eq!(tree.items(), original_tree_items);
        }
    }

    #[test]
    fn get() {
        let mut tree = Tree::new();
        tree.extend((1..8));
        assert_eq!(*tree.get(0).unwrap(), 1);
        assert_eq!(*tree.get(2).unwrap(), 3);
        assert_eq!(*tree.get(4).unwrap(), 5);
        assert_eq!(*tree.get(6).unwrap(), 7);
        assert_eq!(tree.get(7), None);
    }
}
