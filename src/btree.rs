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
pub struct Tree<T: Item>(Arc<Node<T>>);

enum Node<T: Item> {
    Internal {
        id: Option<NodeId>,
        height: u8,
        summary: T::Summary,
        child_summaries: SmallVec<[T::Summary; 2 * TREE_BASE]>,
        child_tree_refs: SmallVec<[TreeRef<T>; 2 * TREE_BASE]>,
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

    pub fn push_item(&mut self, item: T) {
        self.push_tree(Tree(Arc::new(Node::Leaf {
            id: None,
            summary: item.summarize(),
            child_items: SmallVec::from_vec(vec![item]),
        })))
    }
    
    pub fn push_tree(&mut self, tree: Self) {
        let tree_height = tree.height();
        if self.height() < tree_height {
            for tree_ref in tree.child_tree_refs().iter().cloned() {
                self.push_tree_ref(tree_ref, tree_height - 1);
            }
        }
    }
    
    fn push_tree_ref(&mut self, tree_ref: TreeRef<T>, tree_ref_height: u8) {
        
    }

    fn from_child_trees(child_trees: SmallVec<[Tree<T>; 2 * TREE_BASE]>) -> Self {
        let height = child_trees[0].height() + 1;
        let child_summaries = child_trees
            .iter()
            .map(|child| child.summary().clone())
            .collect::<SmallVec<[T::Summary; 2 * TREE_BASE]>>();
        let summary = sum(&child_summaries);
        let child_tree_refs = child_trees
            .into_iter()
            .map(|tree| TreeRef::new(tree))
            .collect::<SmallVec<[TreeRef<T>; 2 * TREE_BASE]>>();

        Tree(Arc::new(Node::Internal {
            id: None,
            height,
            summary,
            child_summaries,
            child_tree_refs,
        }))
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
    
    fn child_tree_refs(&self) -> &[TreeRef<T>] {
        match self.0.as_ref() {
            &Node::Internal { ref child_tree_refs, .. } => child_tree_refs.as_slice(),
            &Node::Leaf { .. } => panic!("Requested child_tree_refs of a leaf node"),
        }
    }    
}

impl<T: Item> TreeRef<T> {
    fn new(tree: Tree<T>) -> Self {
        TreeRef(RwLock::new(TreeRefState::Resident(tree)))
    }
}

impl<T: Item> Clone for TreeRef<T> {
    fn clone(&self) -> Self {
        TreeRef(RwLock::new(self.0.read().clone()))
    }
}

fn sum<T: Default + for<'a> AddAssign<&'a T>>(values: &[T]) -> T {
    let mut sum = T::default();
    for value in values {
        sum += value;
    }
    sum
}
