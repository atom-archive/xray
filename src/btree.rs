use parking_lot::{RwLock, RwLockReadGuard};
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

pub trait NodeStore<T: Item> {
    type ReadError;

    fn read(&self, id: NodeId) -> Result<Node<T>, Self::ReadError>;
    fn write(&mut self, id: NodeId, node: &Node<T>);
}

#[derive(Clone)]
pub struct Tree<T: Item>(Arc<RwLock<TransientNode<T>>>);

enum TransientNode<T: Item> {
    Resident(Node<T>),
    NonResident(NodeId),
}

#[derive(Clone)]
enum Node<T: Item> {
    Internal {
        id: Option<NodeId>,
        height: u8,
        summary: T::Summary,
        child_summaries: SmallVec<[T::Summary; 2 * TREE_BASE]>,
        child_trees: SmallVec<[Tree<T>; 2 * TREE_BASE]>,
    },
    Leaf {
        id: Option<NodeId>,
        summary: T::Summary,
        child_items: SmallVec<[T; 2 * TREE_BASE]>,
    },
}

impl<T: Item> Tree<T> {
    pub fn new() -> Self {
        Tree(Arc::new(RwLock::new(TransientNode::Resident(Node::Leaf {
            id: None,
            summary: T::Summary::default(),
            child_items: SmallVec::new(),
        }))))
    }

    pub fn push_item<S: NodeStore<T>>(&mut self, item: T, db: &S) -> Result<(), S::ReadError> {
        self.push_tree(
            Tree(Arc::new(RwLock::new(TransientNode::Resident(Node::Leaf {
                id: None,
                summary: item.summarize(),
                child_items: SmallVec::from_vec(vec![item]),
            })))),
            db,
        )
    }

    pub fn push_tree<S: NodeStore<T>>(&mut self, other: Self, db: &S) -> Result<(), S::ReadError> {
        // let other_height = other.height();
        // if self.height() < other_height {
        //     for tree in other.child_trees().iter().cloned() {
        //         self.push_tree_recursive(tree, db);
        //     }
        // } else if let Some(split_tree) = self.push_tree_recursive(other, db)? {
        //     *self = Self::from_child_trees(vec![self.clone(), split_tree]);
        // }
        // Ok(())
        unimplemented!()
    }

    fn push_tree_recursive<S>(
        &mut self,
        mut other: Tree<T>,
        db: &S,
    ) -> Result<Option<Tree<T>>, S::ReadError>
    where
        S: NodeStore<T>,
    {
        self.update_node(db, |self_node| {
            other.read_node(db, |other_node| match self_node {
                Node::Internal {
                    height,
                    summary,
                    child_summaries,
                    child_trees,
                    ..
                } => {
                    *summary += other_node.summary();
                    match *height - other_node.height() {
                        0 => {

                        }
                        1 => {

                        }
                        _ => {

                        }
                    }

                    unimplemented!()
                }
                Node::Leaf { child_items, .. } => unimplemented!(),
            })?
        })?

        // *self.summary_mut() += other.summary();
        //
        // let self_height = self.height();
        // let other_height = other.height();
        //
        // if other_height == self_height {
        //     if self_height == 0 {
        //         // self.append_child_items(other.child_items())
        //     } else {
        //         other.read_node(db, |other_node| {
        //             self.append_child_trees(other_node.child_summaries(), other_node.child_trees())
        //         })
        //     }
        // } else if other_height == self_height - 1 && !other.is_underflowing() {
        //     self.append_child_trees(&[other.summary().clone()], &[other])
        // } else {
        //     if let Some(split) = self.last_child_tree_mut().push_tree_recursive(other) {
        //         self.append_child_trees(&[split.summary().clone()], &[split])
        //     } else {
        //         Ok(None)
        //     }
        // }
    }

    fn append_child_trees(
        &mut self,
        summaries: &[T::Summary],
        tree_refs: &[Tree<T>],
    ) -> Option<Tree<T>> {
        // let height = self.height();
        // match Arc::make_mut(&mut self.0) {
        //     &mut Node::Internal {
        //         ref mut summary,
        //         ref mut child_summaries,
        //         ref mut child_trees,
        //         ..
        //     } => {
        //         let child_count = child_trees.len() + tree_refs.len();
        //         if child_count > 2 * TREE_BASE {
        //             let left_summaries: SmallVec<_>;
        //             let right_summaries: SmallVec<_>;
        //             let left_tree_refs;
        //             let right_tree_refs;
        //
        //             let midpoint = (child_count + child_count % 2) / 2;
        //             {
        //                 let mut all_summaries =
        //                     child_summaries.iter().chain(summaries.iter()).cloned();
        //                 left_summaries = all_summaries.by_ref().take(midpoint).collect();
        //                 right_summaries = all_summaries.collect();
        //                 let mut all_tree_refs = child_trees.iter().chain(tree_refs.iter()).cloned();
        //                 left_tree_refs = all_tree_refs.by_ref().take(midpoint).collect();
        //                 right_tree_refs = all_tree_refs.collect();
        //             }
        //             *summary = sum(left_summaries.iter());
        //             *child_summaries = left_summaries;
        //             *child_trees = left_tree_refs;
        //
        //             Some(Tree(Arc::new(Node::Internal {
        //                 id: None,
        //                 height,
        //                 summary: sum(right_summaries.iter()),
        //                 child_summaries: right_summaries,
        //                 child_trees: right_tree_refs,
        //             })))
        //         } else {
        //             *summary += &sum(summaries.iter());
        //             child_summaries.extend(summaries.iter().cloned());
        //             child_trees.extend(tree_refs.iter().cloned());
        //             None
        //         }
        //     }
        //     &mut Node::Leaf { .. } => panic!("Cannot append child tree refs to a leaf node"),
        // }
        unimplemented!()
    }

    fn from_child_trees(child_trees: Vec<Tree<T>>) -> Self {
        // let height = child_trees[0].height() + 1;
        // let child_summaries = child_trees
        //     .iter()
        //     .map(|child| child.summary().clone())
        //     .collect::<SmallVec<[T::Summary; 2 * TREE_BASE]>>();
        // let summary = sum(child_summaries.iter());
        //
        // Tree(Arc::new(RwLock::new(TransientNode::Resident(
        //     Node::Internal {
        //         id: None,
        //         height,
        //         summary,
        //         child_summaries,
        //         child_trees: SmallVec::from_vec(child_trees),
        //     },
        // ))))
        unimplemented!()
    }

    // TODO: Make this method more readable when non-lexical lifetimes are supported.
    fn read_node<S, F, U>(&mut self, db: &S, f: F) -> Result<U, S::ReadError>
    where
        S: NodeStore<T>,
        F: FnOnce(&Node<T>) -> U,
    {
        // Bypass lock acqusition if we hold a unique reference.
        {
            if let Some(lock) = Arc::get_mut(&mut self.0) {
                return Ok(f(lock.get_mut().read_node(db)?))
            }
        }
        // If there is more than one reference:
        let guard = self.0.upgradable_read();
        {
            // First see if the node is already loaded from the database with a read lock.
            if let Some(node) = guard.try_read_node() {
                return Ok(f(node))
            }
        }
        // If the node was not loaded from the database, upgrade to the write lock and load it.
        Ok(f(guard.upgrade().read_node(db)?))
    }

    // TODO: Make this method more readable when non-lexical lifetimes are supported.
    fn update_node<S, F, U>(&mut self, db: &S, f: F) -> Result<U, S::ReadError>
    where
        S: NodeStore<T>,
        F: FnOnce(&mut Node<T>) -> U,
    {
        // Bypass lock acqusition and grab a mutable Node reference if we hold a unique reference.
        {
            if let Some(lock) = Arc::get_mut(&mut self.0) {
                return Ok(f(lock.get_mut().read_node(db)?))
            }
        }
        // If there is more than one reference we need to clone our node in order to mutate it.
        let mut new_node = self.clone_node(db)?;
        new_node.clear_id();
        let result = f(&mut new_node);
        *self = Tree(Arc::new(RwLock::new(TransientNode::Resident(new_node))));
        Ok(result)
    }

    // TODO: Inline this method when non-lexical lifetimes are supported.
    fn clone_node<S: NodeStore<T>>(&self, db: &S) -> Result<Node<T>, S::ReadError> {
        let guard = self.0.upgradable_read();
        {
            if let Some(node) = guard.try_read_node() {
                return Ok(node.clone());
            }
        }
        Ok(guard.upgrade().read_node(db)?.clone())
    }
}

impl<T: Item> TransientNode<T> {
    fn read_node<S: NodeStore<T>>(&mut self, db: &S) -> Result<&mut Node<T>, S::ReadError> {
        match self {
            TransientNode::Resident(ref mut node) => return Ok(node),
            TransientNode::NonResident(id) => *self = TransientNode::Resident(db.read(*id)?),
        }
        match self {
            TransientNode::Resident(node) => Ok(node),
            TransientNode::NonResident(_) => unreachable!(),
        }
    }

    fn try_read_node(&self) -> Option<&Node<T>> {
        match self {
            TransientNode::Resident(node) => Some(node),
            TransientNode::NonResident(_) => None,
        }
    }
}

impl<T: Item> Node<T> {
    fn clear_id(&mut self) {
        match self {
            Node::Internal { id, .. } => *id = None,
            Node::Leaf { id, .. } => *id = None,
        }
    }

    fn height(&self) -> u8 {
        match self {
            Node::Internal { height, .. } => *height,
            Node::Leaf { .. } => 0,
        }
    }

    fn summary(&self) -> &T::Summary {
        match self {
            Node::Internal { summary, .. } => summary,
            Node::Leaf { summary, .. } => summary,
        }
    }

    fn child_summaries(&self) -> &[T::Summary] {
        match self {
            Node::Internal {
                child_summaries, ..
            } => child_summaries.as_slice(),
            Node::Leaf { .. } => panic!("Leaf nodes have no child summaries"),
        }
    }

    fn child_trees(&self) -> &[Tree<T>] {
        match self {
            Node::Internal { child_trees, .. } => child_trees.as_slice(),
            Node::Leaf { .. } => panic!("Leaf nodes have no child trees"),
        }
    }

    fn child_items(&self) -> &[T] {
        match self {
            Node::Leaf { child_items, .. } => child_items.as_slice(),
            Node::Internal { .. } => panic!("Internal nodes have no child items"),
        }
    }

    fn is_underflowing(&self) -> bool {
        match self {
            Node::Internal { child_trees, .. } => child_trees.len() < TREE_BASE,
            Node::Leaf { child_items, .. } => child_items.len() < TREE_BASE,
        }
    }
}

fn sum<'a, T, I>(iter: I) -> T
where
    T: 'a + Default + AddAssign<&'a T>,
    I: Iterator<Item = &'a T>,
{
    let mut sum = T::default();
    for value in iter {
        sum += value;
    }
    sum
}
