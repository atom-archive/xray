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

#[derive(Clone, Debug)]
struct Tree<T: Item> {
    root: TreeRef<T>,
    summary: T::Summary,
    height: u8,
}

#[derive(Clone, Debug)]
enum TreeRef<T: Item> {
    Cached(Arc<Node<T>>),
    NotCached(NodeId),
}

#[derive(Debug)]
enum Node<T: Item> {
    Internal {
        id: Option<NodeId>,
        children: SmallVec<[Tree<T>; 2 * TREE_BASE]>,
    },
    Leaf {
        id: Option<NodeId>,
        children: SmallVec<[T; 2 * TREE_BASE]>,
    },
}

impl<T: Item> Tree<T> {
    pub fn new() -> Self {
        Self::from_children(SmallVec::new())
    }

    fn from_children(children: SmallVec<[Tree<T>; 2 * TREE_BASE]>) -> Self {
        let summary = Self::summarize_children(&children);
        let height = children.get(0).map(|c| c.height).unwrap_or(0) + 1;
        Tree {
            root: TreeRef::Cached(Arc::new(Node::Internal { id: None, children })),
            summary,
            height,
        }
    }

    fn summarize_children(children: &[Tree<T>]) -> T::Summary {
        let mut summary = T::Summary::default();
        for child in children {
            summary += &child.summary;
        }
        summary
    }
}
