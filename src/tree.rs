use std::fmt;
use std::sync::Arc;
use std::clone::Clone;

const MIN_CHILDREN: usize = 4;
const MAX_CHILDREN: usize = 8;

pub trait TreeItem: Clone + Eq + fmt::Debug {
    type Summary: Summary;

    fn summarize(&self) -> Self::Summary;
}

pub trait Summary: Clone + Eq + fmt::Debug {
    fn accumulate<'a, T: IntoIterator<Item=&'a Self>>(summaries: T) -> Self where Self: 'a + Sized;
}

#[derive(Clone, Eq, PartialEq)]
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

impl<T: TreeItem> fmt::Debug for Tree<T> {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        fmt.debug_tuple("Tree")
            .field(self.0.as_ref())
            .finish()
    }
}

impl<T: TreeItem> From<T> for Tree<T> {
    fn from(value: T) -> Self {
        Self::new(Node::Leaf {
            summary: value.summarize(),
            value: value
        })
    }
}

impl<T: TreeItem> Tree<T> {
    fn new(node: Node<T>) -> Self {
        Tree(Arc::new(node))
    }

    pub fn empty() -> Self {
        Self::new(Node::Empty)
    }

    fn from_children(children: Vec<Self>) -> Self {
        let summary = T::Summary::accumulate(children.iter().map(|tree| tree.get_summary()));
        let height = children[0].height() + 1;
        Self::new(Node::Internal { summary, children, height })
    }

    fn merge_nodes(left_children: &[Tree<T>], right_children: &[Tree<T>]) -> Self {
        let child_count = left_children.len() + right_children.len();
        if child_count <= MAX_CHILDREN {
            Self::from_children([left_children, right_children].concat())
        } else {
            let midpoint = (child_count + child_count % 2) / 2;
            let mut children_iter = left_children.iter().chain(right_children.iter()).cloned();
            Self::from_children(vec![
                Self::from_children(children_iter.by_ref().take(midpoint).collect()),
                Self::from_children(children_iter.collect())
            ])
        }
    }

    pub fn concat(left: Self, right: Self) -> Self {
        use std::cmp::Ordering;

        let left_height = left.height();
        let right_height = right.height();

        if left_height == 0 { // left is empty
            return right;
        }

        if right_height == 0 { // right is empty
            return left;
        }

        match left_height.cmp(&right_height) {
            Ordering::Less => {
                let right_children = right.get_children();
                if left_height == right_height - 1 && !left.underflowing() {
                    Tree::merge_nodes(&[left], right_children)
                } else {
                    let (first_right_child, right_children) = right_children.split_first().unwrap();
                    let new_left = Tree::concat(left, first_right_child.clone());
                    if new_left.height() == right_height - 1 {
                        Tree::merge_nodes(&[new_left], right_children)
                    } else {
                        Tree::merge_nodes(new_left.get_children(), right_children)
                    }
                }
            },
            Ordering::Equal => {
                if left_height == 1 { // leaf nodes
                    Tree::from_children(vec![left, right])
                } else {
                    if left.get_child_count() + right.get_child_count() <= MAX_CHILDREN {
                        Tree::merge_nodes(left.get_children(), right.get_children())
                    } else {
                        Tree::from_children(vec![left, right])
                    }
                }
            },
            Ordering::Greater => {
                let left_children = left.get_children();
                if right_height == left_height - 1 && !right.underflowing() {
                    Tree::merge_nodes(left_children, &[right])
                } else {
                    let (last, left_children) = left_children.split_last().unwrap();
                    let new_right = Tree::concat(last.clone(), right);
                    if new_right.height() == left_height - 1 {
                        Tree::merge_nodes(left_children, &[new_right])
                    } else {
                        Tree::merge_nodes(left_children, new_right.get_children())
                    }
                }
            }
        }
    }

    fn height(&self) -> u16 {
        match self.0.as_ref() {
            &Node::Empty => 0,
            &Node::Leaf { .. } => 1,
            &Node::Internal { height, ..} => height
        }
    }

    fn get_children(&self) -> &[Tree<T>] {
        match self.0.as_ref() {
            &Node::Internal { ref children, .. } => children.as_slice(),
            _ => panic!("Requested children of a non-internal node")
        }
    }

    fn get_child_count(&self) -> usize {
        match self.0.as_ref() {
            &Node::Internal { ref children, .. } => children.len(),
            _ => 0
        }
    }

    fn get_summary(&self) -> &T::Summary {
        match self.0.as_ref() {
            &Node::Empty => panic!("Requested a summary of an empty node"),
            &Node::Leaf { ref summary, .. } => summary,
            &Node::Internal { ref summary, .. } => summary,
        }
    }

    fn underflowing(&self) -> bool {
        match self.0.as_ref() {
            &Node::Internal { ref children, ..} => children.len() < MIN_CHILDREN,
            _ => false
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
        fn accumulate<'a, T: IntoIterator<Item=&'a Self>>(summaries: T) -> Self where Self: 'a + Sized {
            summaries.into_iter().sum()
        }
    }

    #[test]
    fn concat() {
        assert_eq!(
            Tree::concat(Tree::<u16>::empty(), Tree::<u16>::empty()),
            Tree::<u16>::empty()
        );

        assert_eq!(
            Tree::concat(Tree::empty(), Tree::from(1)),
            Tree::from(1)
        );

        assert_eq!(
            Tree::concat(Tree::from(1), Tree::empty()),
            Tree::from(1)
        );

        assert_eq!(
            Tree::concat(Tree::concat(Tree::from(1), Tree::from(2)), Tree::from(3)),
            Tree::concat(Tree::from(1), Tree::concat(Tree::from(2), Tree::from(3)))
        );
    }
}
