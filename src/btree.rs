use parking_lot::RwLock;
use smallvec::SmallVec;
use std::fmt;
use std::ops::AddAssign;
use std::sync::Arc;

const TREE_BASE: usize = 16;
type NodeId = usize;

pub trait Item: Clone + Eq + fmt::Debug {
    type Summary: for<'a> AddAssign<&'a Self::Summary> + Default + Eq + Clone + fmt::Debug;

    fn summarize(&self) -> Self::Summary;
}

#[derive(Clone)]
struct Tree<T: Item>(Arc<Node<T>>);

enum Node<T: Item> {
    Internal {
        id: Option<NodeId>,
        height: u8,
        summary: T::Summary,
        child_summaries: SmallVec<[T::Summary; 2 * TREE_BASE]>,
        child_trees: SmallVec<[TreeRef<T>; 2 * TREE_BASE]>,
    },
    Leaf {
        id: Option<NodeId>,
        summary: T::Summary,
        child_items: SmallVec<[T; 2 * TREE_BASE]>,
    },
}

struct TreeRef<T: Item>(RwLock<TreeRefState<T>>);

#[derive(Clone)]
enum TreeRefState<T: Item> {
    Resident(Tree<T>),
    NonResident(NodeId),
}

impl<T: Item> Tree<T> {
    pub fn new() -> Self {
        Tree(Arc::new(Node::Leaf {
            id: None,
            summary: T::Summary::default(),
            child_items: SmallVec::new(),
        }))
    }

    fn from_child_trees(child_trees: SmallVec<[Tree<T>; 2 * TREE_BASE]>) -> Self {
        let height = child_trees[0].height() + 1;
        let child_summaries = child_trees
            .iter()
            .map(|child| child.summary().clone())
            .collect::<SmallVec<[T::Summary; 2 * TREE_BASE]>>();
        let summary = sum(&child_summaries);
        let child_trees = child_trees
            .into_iter()
            .map(|tree| TreeRef::new(tree))
            .collect::<SmallVec<[TreeRef<T>; 2 * TREE_BASE]>>();

        Tree(Arc::new(Node::Internal {
            id: None,
            height,
            summary,
            child_summaries,
            child_trees,
        }))
    }

    fn summarize_child_trees(child_trees: &[Tree<T>]) -> T::Summary {
        let mut summary = T::Summary::default();
        for child in child_trees {
            summary += &child.summary();
        }
        summary
    }

    fn height(&self) -> u8 {
        match self.0.as_ref() {
            &Node::Internal { height, .. } => height,
            &Node::Leaf { .. } => 0,
        }
    }

    fn summary(&self) -> &T::Summary {
        match self.0.as_ref() {
            &Node::Internal { ref summary, .. } => summary,
            &Node::Leaf { ref summary, .. } => summary,
        }
    }
}

impl<T: Item> TreeRef<T> {
    fn new(tree: Tree<T>) -> Self {
        TreeRef(RwLock::new(TreeRefState::Resident(tree)))
    }
}

fn sum<T: Default + for<'a> AddAssign<&'a T>>(values: &[T]) -> T {
    let mut sum = T::default();
    for value in values {
        sum += value;
    }
    sum
}