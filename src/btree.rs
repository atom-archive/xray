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
    type ReadError: fmt::Debug;

    fn get(&mut self, id: NodeId) -> Result<&Node<T>, Self::ReadError>;
}

#[derive(Clone)]
pub enum Tree<T: Item> {
    Resident(Arc<Node<T>>),
    NonResident(NodeId),
}

#[derive(Clone)]
enum Node<T: Item> {
    Internal {
        height: u8,
        summary: T::Summary,
        child_summaries: SmallVec<[T::Summary; 2 * TREE_BASE]>,
        child_trees: SmallVec<[Tree<T>; 2 * TREE_BASE]>,
    },
    Leaf {
        summary: T::Summary,
        child_items: SmallVec<[T; 2 * TREE_BASE]>,
    },
}

impl<T: Item> Tree<T> {
    pub fn new() -> Self {
        Tree::Resident(Arc::new(Node::Leaf {
            summary: T::Summary::default(),
            child_items: SmallVec::new(),
        }))
    }

    pub fn push_item<S: NodeStore<T>>(&mut self, item: T, db: &mut S) -> Result<(), S::ReadError> {
        self.push_tree(
            Tree::Resident(Arc::new(Node::Leaf {
                summary: item.summarize(),
                child_items: SmallVec::from_vec(vec![item]),
            })),
            db,
        )
    }

    pub fn push_tree<S: NodeStore<T>>(
        &mut self,
        other: Self,
        db: &mut S,
    ) -> Result<(), S::ReadError> {
        unimplemented!()
        // let other_height = other.height(db)?;
        // if self.height(db)? < other_height {
        //     for tree in other.child_trees(db)?.clone() {
        //         self.push_tree_recursive(tree, db);
        //     }
        // } else if let Some(split_tree) = self.push_tree_recursive(other, db)? {
        //     *self = Self::from_child_trees(vec![self.clone(), split_tree]);
        // }
        // Ok(())
    }

    fn push_tree_recursive<S>(
        &mut self,
        mut other: Tree<T>,
        db: &mut S,
    ) -> Result<Option<Tree<T>>, S::ReadError>
    where
        S: NodeStore<T>,
    {
        match self.make_mut_node(db)? {
            Node::Internal {
                height,
                summary,
                child_summaries,
                child_trees,
            } => {
                let height_delta;
                let mut summaries_to_append = SmallVec::<[T::Summary; 2 * TREE_BASE]>::new();
                let mut trees_to_append = SmallVec::<[Tree<T>; 2 * TREE_BASE]>::new();

                {
                    let other_node = other.node(db)?;
                    *summary += other_node.summary();
                    height_delta = *height - other_node.height();
                    if height_delta == 0 {
                        summaries_to_append.extend(other_node.child_summaries().iter().cloned());
                        trees_to_append.extend(other_node.child_trees().iter().cloned());
                    } else if height_delta == 1 {
                        summaries_to_append.push(other_node.summary().clone());
                    }
                }

                if height_delta == 1 {
                    trees_to_append.push(other)
                } else if height_delta > 1 {
                    let tree_to_append = child_trees
                        .last_mut()
                        .unwrap()
                        .push_tree_recursive(other, db)?;

                    if let Some(tree) = tree_to_append {
                        summaries_to_append.push(tree.summary(db).unwrap().clone());
                        trees_to_append.push(tree);
                    }
                }

                let child_count = child_trees.len() + trees_to_append.len();
                if child_count > 2 * TREE_BASE {
                    let left_summaries;
                    let right_summaries: SmallVec<_>;
                    let left_trees;
                    let right_trees;

                    let midpoint = (child_count + child_count % 2) / 2;
                    {
                        let mut all_summaries = child_summaries
                            .iter()
                            .chain(summaries_to_append.iter())
                            .cloned();
                        left_summaries = all_summaries.by_ref().take(midpoint).collect();
                        right_summaries = all_summaries.collect();
                        let mut all_trees =
                            child_trees.iter().chain(trees_to_append.iter()).cloned();
                        left_trees = all_trees.by_ref().take(midpoint).collect();
                        right_trees = all_trees.collect();
                    }
                    *child_summaries = left_summaries;
                    *child_trees = left_trees;
                    *summary = sum(child_summaries.iter());

                    Ok(Some(Tree::Resident(Arc::new(Node::Internal {
                        height: *height,
                        summary: sum(right_summaries.iter()),
                        child_summaries: right_summaries,
                        child_trees: right_trees,
                    }))))
                } else {
                    *summary += &sum(summaries_to_append.iter());
                    child_summaries.extend(summaries_to_append);
                    child_trees.extend(trees_to_append);
                    Ok(None)
                }
            }
            Node::Leaf { summary, child_items, .. } => {
                let other_node = other.node(db)?;

                let child_count = child_items.len() + other_node.child_items().len();
                if child_count > 2 * TREE_BASE {
                    let left_items;
                    let right_items: SmallVec<[T; 2 * TREE_BASE]>;

                    let midpoint = (child_count + child_count % 2) / 2;
                    {
                        let mut all_items = child_items
                            .iter()
                            .chain(other_node.child_items().iter())
                            .cloned();
                        left_items = all_items.by_ref().take(midpoint).collect();
                        right_items = all_items.collect();
                    }
                    *child_items = left_items;
                    *summary = sum(child_items.iter().map(|item| item.summarize()).by_ref());

                    Ok(Some(Tree::Resident(Arc::new(Node::Leaf {
                        summary: sum(right_items.iter().map(|item| item.summarize()).by_ref()),
                        child_items: right_items,
                    }))))
                } else {
                    *summary += other_node.summary();
                    child_items.extend(other_node.child_items().iter().cloned());
                    Ok(None)
                }
            }
        }
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

    fn make_mut_node<S: NodeStore<T>>(&mut self, db: &mut S) -> Result<&mut Node<T>, S::ReadError> {
        if let Tree::NonResident(node_id) = self {
            *self = Tree::Resident(Arc::new(db.get(*node_id)?.clone()));
        }

        match self {
            Tree::Resident(node) => Ok(Arc::make_mut(node)),
            Tree::NonResident(node_id) => unreachable!(),
        }
    }

    fn node<'a, S: NodeStore<T>>(&'a self, db: &'a mut S) -> Result<&'a Node<T>, S::ReadError> {
        match self {
            Tree::Resident(node) => Ok(node),
            Tree::NonResident(node_id) => db.get(*node_id),
        }
    }

    fn height<S: NodeStore<T>>(&self, db: &mut S) -> Result<u8, S::ReadError> {
        match self {
            Tree::Resident(node) => Ok(node.height()),
            Tree::NonResident(node_id) => Ok(db.get(*node_id)?.height()),
        }
    }

    fn summary<'a, S: NodeStore<T>>(
        &'a self,
        db: &'a mut S,
    ) -> Result<&'a T::Summary, S::ReadError> {
        match self {
            Tree::Resident(node) => Ok(node.summary()),
            Tree::NonResident(node_id) => Ok(db.get(*node_id)?.summary()),
        }
    }

    fn child_trees<'a, S: NodeStore<T>>(
        &'a self,
        db: &'a mut S,
    ) -> Result<&'a SmallVec<[Tree<T>; 2 * TREE_BASE]>, S::ReadError> {
        match self {
            Tree::Resident(node) => Ok(node.child_trees()),
            Tree::NonResident(node_id) => Ok(db.get(*node_id)?.child_trees()),
        }
    }
}

impl<T: Item> Node<T> {
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

    fn child_trees(&self) -> &SmallVec<[Tree<T>; 2 * TREE_BASE]> {
        match self {
            Node::Internal { child_trees, .. } => child_trees,
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
