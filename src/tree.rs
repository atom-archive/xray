use std::fmt;
use std::sync::Arc;
use std::clone::Clone;

const MIN_CHILDREN: usize = 2;
const MAX_CHILDREN: usize = 4;

pub trait TreeItem: Clone + Eq + fmt::Debug {
    type Summary: Summary;

    fn summarize(&self) -> Self::Summary;
}

pub trait Summary: Clone + Eq + fmt::Debug {
    fn accumulate(&mut self, other: &Self);
}

pub trait Dimension: Default + Ord {
    type Summary: Summary;

    fn from_summary(summary: &Self::Summary) -> Self;

    fn accumulate(&self, other: &Self) -> Self;
}

#[derive(Clone, Eq, PartialEq, Debug)]
pub struct Tree<T: TreeItem>(Arc<Node<T>>);

#[derive(Clone, Eq, PartialEq, Debug)]
pub enum Node<T: TreeItem> {
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

impl<T: TreeItem> From<T> for Tree<T> {
    fn from(value: T) -> Self {
        Tree(Arc::new(Node::Leaf {
            summary: value.summarize(),
            value: value
        }))
    }
}

impl<T: TreeItem> Extend<T> for Tree<T> {
    fn extend<I: IntoIterator<Item=T>>(&mut self, items: I) {
        for item in items.into_iter() {
            self.push(Self::from(item));
        }
    }
}

impl<'a, T: TreeItem> Tree<T> {
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

    // This should only be called on the root.
    fn push(&mut self, other: Tree<T>) {
        let self_height = self.height();
        let other_height = other.height();

        // Other is empty.
        if other_height == 0 {
            return;
        }

        // Self is empty.
        if self_height == 0 {
            *self = other;
            return;
        }

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
                    *children = left_children;
                    *summary = Self::summarize_children(children);
                    Some(Tree::from_children(right_children))
                } else {
                    summary.accumulate(&Self::summarize_children(new_children));
                    children.extend(new_children.iter().cloned());
                    None
                }
            },
            _ => panic!("Tried to append children to a non-internal node")
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

    fn underflowing(&self) -> bool {
        match self.0.as_ref() {
            &Node::Internal { ref children, ..} => children.len() < MIN_CHILDREN,
            _ => false
        }
    }

    fn height(&self) -> u16 {
        match self.0.as_ref() {
            &Node::Empty => 0,
            &Node::Leaf { .. } => 1,
            &Node::Internal { height, ..} => height
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    impl TreeItem for u16 {
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

    #[test]
    fn push() {
        let mut tree1 = Tree::new();
        tree1.push(Tree::new());
        assert_eq!(tree1, Tree::new());

        tree1.push(Tree::from(1));
        assert_eq!(tree1, Tree::from(1));

        tree1.push(Tree::from(2));

        // let mut tree2 = Tree:

        // assert_eq!(
        //     Tree::concat(Tree::new(), Tree::from(1)),
        //     Tree::from(1)
        // );
        //
        // assert_eq!(
        //     Tree::concat(Tree::from(1), Tree::new()),
        //     Tree::from(1)
        // );
        //
        // assert_eq!(
        //     Tree::concat(Tree::concat(Tree::from(1), Tree::from(2)), Tree::from(3)),
        //     Tree::concat(Tree::from(1), Tree::concat(Tree::from(2), Tree::from(3)))
        // );
    }

    #[test]
    fn get() {
        let mut tree = Tree::new();
        tree.extend(vec![1, 2, 3, 4, 5, 6, 7]);
        assert_eq!(*tree.get(0).unwrap(), 1);
        assert_eq!(*tree.get(2).unwrap(), 3);
        assert_eq!(*tree.get(4).unwrap(), 5);
        assert_eq!(*tree.get(6).unwrap(), 7);
        assert_eq!(tree.get(7), None);
    }
}
