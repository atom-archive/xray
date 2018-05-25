use smallvec::SmallVec;
use std::fmt;
use std::marker::PhantomData;
use std::ops::{Add, AddAssign};
use std::sync::Arc;

const TREE_BASE: usize = 2;
type NodeId = usize;

pub trait Item: Clone + Eq + fmt::Debug {
    type Summary: for<'a> AddAssign<&'a Self::Summary> + Default + Clone + fmt::Debug;

    fn summarize(&self) -> Self::Summary;
}

pub trait Dimension: for<'a> Add<&'a Self, Output = Self> + Ord + Clone + fmt::Debug {
    type Summary: Default;

    fn from_summary(summary: &Self::Summary) -> Self;

    fn default() -> Self {
        Self::from_summary(&Self::Summary::default())
    }
}

pub trait NodeStore<T: Item> {
    type ReadError: fmt::Debug;

    fn get(&mut self, id: NodeId) -> Result<&Node<T>, Self::ReadError>;
}

#[derive(Clone, Debug)]
pub enum Tree<T: Item> {
    Resident(Arc<Node<T>>),
    NonResident(NodeId),
}

#[derive(Clone, Debug)]
pub enum Node<T: Item> {
    Internal {
        height: u8,
        summary: T::Summary,
        child_summaries: SmallVec<[T::Summary; 2 * TREE_BASE]>,
        child_trees: SmallVec<[Tree<T>; 2 * TREE_BASE]>,
    },
    Leaf {
        summary: T::Summary,
        items: SmallVec<[T; 2 * TREE_BASE]>,
    },
}

pub struct Cursor<T: Item> {
    tree: Tree<T>,
    stack: SmallVec<[(Tree<T>, usize); 16]>,
    summary: T::Summary,
    did_seek: bool,
}

#[derive(Eq, PartialEq)]
pub enum SeekBias {
    Left,
    Right,
}

#[derive(Debug)]
pub struct NullNodeStoreReadError;

pub struct NullNodeStore<T: Item>(PhantomData<T>);

impl<T: Item> Tree<T> {
    pub fn new() -> Self {
        Tree::Resident(Arc::new(Node::Leaf {
            summary: T::Summary::default(),
            items: SmallVec::new(),
        }))
    }

    fn cursor(&self) -> Cursor<T> {
        Cursor::new(self.clone())
    }

    fn extend<I, S>(&mut self, iter: I, db: &mut S) -> Result<(), S::ReadError>
    where
        I: IntoIterator<Item = T>,
        S: NodeStore<T>,
    {
        let mut leaf: Option<Node<T>> = None;

        for item in iter {
            if leaf.is_some() && leaf.as_ref().unwrap().items().len() == 2 * TREE_BASE {
                self.push_tree(Tree::Resident(Arc::new(leaf.take().unwrap())), db)?;
            }

            if leaf.is_none() {
                leaf = Some(Node::Leaf::<T> {
                    summary: T::Summary::default(),
                    items: SmallVec::new(),
                });
            }

            leaf.as_mut().unwrap().items_mut().push(item);
        }

        if leaf.is_some() {
            self.push_tree(Tree::Resident(Arc::new(leaf.take().unwrap())), db)?;
        }

        Ok(())
    }

    pub fn push<S: NodeStore<T>>(&mut self, item: T, db: &mut S) -> Result<(), S::ReadError> {
        self.push_tree(
            Tree::from_child_trees(
                vec![Tree::Resident(Arc::new(Node::Leaf {
                    summary: item.summarize(),
                    items: SmallVec::from_vec(vec![item]),
                }))],
                db,
            )?,
            db,
        )
    }

    pub fn push_tree<S: NodeStore<T>>(
        &mut self,
        other: Self,
        db: &mut S,
    ) -> Result<(), S::ReadError> {
        let other_height = other.height(db)?;
        if self.height(db)? < other_height {
            for tree in other.child_trees(db)?.clone() {
                self.push_tree_recursive(tree, db)?;
            }
        } else if let Some(split_tree) = self.push_tree_recursive(other, db)? {
            *self = Self::from_child_trees(vec![self.clone(), split_tree], db)?;
        }
        Ok(())
    }

    fn push_tree_recursive<S>(
        &mut self,
        other: Tree<T>,
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
                    } else if height_delta == 1 && !other_node.is_underflowing() {
                        summaries_to_append.push(other_node.summary().clone());
                    }
                }

                if height_delta == 1 && summaries_to_append.len() > 0 {
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
            Node::Leaf { summary, items, .. } => {
                let other_node = other.node(db)?;

                let child_count = items.len() + other_node.items().len();
                if child_count > 2 * TREE_BASE {
                    let left_items;
                    let right_items: SmallVec<[T; 2 * TREE_BASE]>;

                    let midpoint = (child_count + child_count % 2) / 2;
                    {
                        let mut all_items = items.iter().chain(other_node.items().iter()).cloned();
                        left_items = all_items.by_ref().take(midpoint).collect();
                        right_items = all_items.collect();
                    }
                    *items = left_items;
                    *summary = sum_owned(items.iter().map(|item| item.summarize()));
                    Ok(Some(Tree::Resident(Arc::new(Node::Leaf {
                        summary: sum_owned(right_items.iter().map(|item| item.summarize())),
                        items: right_items,
                    }))))
                } else {
                    *summary += other_node.summary();
                    items.extend(other_node.items().iter().cloned());
                    Ok(None)
                }
            }
        }
    }

    fn from_child_trees<S>(child_trees: Vec<Tree<T>>, db: &mut S) -> Result<Self, S::ReadError>
    where
        S: NodeStore<T>,
    {
        let height = child_trees[0].height(db)? + 1;
        let mut child_summaries = SmallVec::new();
        for child in &child_trees {
            child_summaries.push(child.summary(db)?.clone());
        }
        let summary = sum(child_summaries.iter());
        Ok(Tree::Resident(Arc::new(Node::Internal {
            height,
            summary,
            child_summaries,
            child_trees: SmallVec::from_vec(child_trees),
        })))
    }

    fn make_mut_node<S: NodeStore<T>>(&mut self, db: &mut S) -> Result<&mut Node<T>, S::ReadError> {
        if let Tree::NonResident(node_id) = self {
            *self = Tree::Resident(Arc::new(db.get(*node_id)?.clone()));
        }

        match self {
            Tree::Resident(node) => Ok(Arc::make_mut(node)),
            Tree::NonResident(_) => unreachable!(),
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

    #[cfg(test)]
    pub fn items<S: NodeStore<T>>(&self, db: &mut S) -> Result<Vec<T>, S::ReadError> {
        let mut items = Vec::new();
        let mut cursor = self.cursor();
        cursor.descend_to_start(self.clone(), db)?;
        loop {
            if let Some(item) = cursor.item(db)? {
                items.push(item.clone());
            } else {
                break;
            }
            cursor.next(db)?;
        }
        Ok(items)
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

    fn items(&self) -> &SmallVec<[T; 2 * TREE_BASE]> {
        match self {
            Node::Leaf { items, .. } => items,
            Node::Internal { .. } => panic!("Internal nodes have no items"),
        }
    }

    fn items_mut(&mut self) -> &mut SmallVec<[T; 2 * TREE_BASE]> {
        match self {
            Node::Leaf { items, .. } => items,
            Node::Internal { .. } => panic!("Internal nodes have no items"),
        }
    }

    fn is_underflowing(&self) -> bool {
        match self {
            Node::Internal { child_trees, .. } => child_trees.len() < TREE_BASE,
            Node::Leaf { items, .. } => items.len() < TREE_BASE,
        }
    }
}

impl<T: Item> Cursor<T> {
    fn new(tree: Tree<T>) -> Self {
        Self {
            tree,
            stack: SmallVec::new(),
            summary: T::Summary::default(),
            did_seek: false,
        }
    }

    fn reset(&mut self) {
        self.did_seek = false;
        self.stack.truncate(0);
        self.summary = T::Summary::default();
    }

    pub fn item<'a, S: 'a + NodeStore<T>>(
        &'a self,
        db: &'a mut S,
    ) -> Result<Option<&'a T>, S::ReadError> {
        assert!(self.did_seek, "Must seek before calling this method");
        if let Some((subtree, index)) = self.stack.last() {
            eprintln!("item {} {:#?} ", index, subtree);

            match subtree.node(db)? {
                Node::Leaf { items, .. } => Ok(Some(&items[*index])),
                _ => unreachable!(),
            }
        } else {
            Ok(None)
        }
    }

    pub fn next<S: NodeStore<T>>(&mut self, db: &mut S) -> Result<(), S::ReadError> {
        assert!(self.did_seek, "Must seek before calling this method");

        while self.stack.len() > 0 {
            let new_subtree = {
                let (subtree, index) = self.stack.last_mut().unwrap();
                match subtree.node(db)? {
                    Node::Internal { child_trees, .. } => {
                        *index += 1;
                        child_trees.get(*index).cloned()
                    }
                    Node::Leaf { items, .. } => {
                        self.summary += &items[*index].summarize();
                        eprintln!("incrementing index on leaf {:?}", index);
                        *index += 1;
                        if *index < items.len() {
                            eprintln!("stop");
                            return Ok(());
                        } else {
                            None
                        }
                    }
                }
            };

            if let Some(subtree) = new_subtree {
                eprintln!("descend to start");
                self.descend_to_start(subtree, db)?;
                break;
            } else {
                eprintln!("pop stack");
                self.stack.pop();
            }
        }

        Ok(())
    }

    fn descend_to_start<S>(&mut self, mut subtree: Tree<T>, db: &mut S) -> Result<(), S::ReadError>
    where
        S: NodeStore<T>,
    {
        self.did_seek = true;
        loop {
            self.stack.push((subtree.clone(), 0));
            subtree = match subtree.node(db)? {
                Node::Internal { child_trees, .. } => child_trees[0].clone(),
                Node::Leaf { .. } => {
                    return Ok(());
                }
            }
        }
    }

    pub fn seek<D, S>(&mut self, pos: &D, bias: SeekBias, db: &mut S) -> Result<(), S::ReadError>
    where
        D: Dimension<Summary = T::Summary>,
        S: NodeStore<T>,
    {
        self.reset();
        self.seek_internal(pos, bias, db, None)
    }

    pub fn slice<D, S>(
        &mut self,
        end: &D,
        bias: SeekBias,
        db: &mut S,
    ) -> Result<Tree<T>, S::ReadError>
    where
        D: Dimension<Summary = T::Summary>,
        S: NodeStore<T>,
    {
        let mut slice = Tree::new();
        self.seek_internal(end, bias, db, Some(&mut slice));
        Ok(slice)
    }

    fn seek_internal<D, S>(
        &mut self,
        pos: &D,
        bias: SeekBias,
        db: &mut S,
        mut slice: Option<&mut Tree<T>>,
    ) -> Result<(), S::ReadError>
    where
        D: Dimension<Summary = T::Summary>,
        S: NodeStore<T>,
    {
        let mut containing_subtree = None;
        let mut slice_subtrees = SmallVec::<[Tree<T>; 2 * TREE_BASE]>::new();

        if self.did_seek {
            'outer: while self.stack.len() > 0 {
                {
                    let (parent_subtree, index) = self.stack.last_mut().unwrap();
                    match parent_subtree.node(db)? {
                        Node::Internal {
                            child_summaries,
                            child_trees,
                            ..
                        } => {
                            *index += 1;
                            while *index < child_summaries.len() {
                                let child_tree = &child_trees[*index];
                                let child_summary = &child_summaries[*index];
                                let child_end = D::from_summary(&self.summary)
                                    + &D::from_summary(&child_summary);

                                if *pos > child_end
                                    || (*pos == child_end && bias == SeekBias::Right)
                                {
                                    self.summary += child_summary;
                                    slice_subtrees.push(child_tree.clone());
                                    *index += 1;
                                } else {
                                    containing_subtree = Some(child_tree.clone());
                                }
                            }
                        }
                        Node::Leaf { items, .. } => {
                            let mut slice_items = SmallVec::<[T; 2 * TREE_BASE]>::new();
                            let mut slice_items_summary = T::Summary::default();

                            while *index < items.len() {
                                let item = &items[*index];
                                let item_summary = item.summarize();
                                let item_end = D::from_summary(&self.summary)
                                    + &D::from_summary(&item_summary);

                                if *pos > item_end || (*pos == item_end && bias == SeekBias::Right)
                                {
                                    self.summary += &item_summary;
                                    slice_items.push(item.clone());
                                    slice_items_summary += &item_summary;
                                    *index += 1;
                                } else {
                                    slice_subtrees.push(Tree::Resident(Arc::new(Node::Leaf {
                                        summary: slice_items_summary,
                                        items: slice_items,
                                    })));
                                    break 'outer;
                                }
                            }

                            slice_subtrees.push(Tree::Resident(Arc::new(Node::Leaf {
                                summary: slice_items_summary,
                                items: slice_items,
                            })));
                        }
                    }
                }
                if containing_subtree.is_some() {
                    break;
                } else {
                    self.stack.pop();
                }
            }
        } else {
            containing_subtree = Some(self.tree.clone());
        }

        if let Some (mut subtree) = containing_subtree {
            loop {
                let mut next_subtree = None;
                match subtree.node(db)? {
                    Node::Internal {
                        child_summaries,
                        child_trees,
                        ..
                    } => {
                        for (index, child_summary) in child_summaries.iter().enumerate() {
                            let child_end =
                                D::from_summary(&self.summary) + &D::from_summary(child_summary);
                            if *pos > child_end || (*pos == child_end && bias == SeekBias::Right) {
                                self.summary += child_summary;
                                if slice.is_some() {
                                    slice_subtrees.push(child_trees[index].clone());
                                }
                            } else {
                                self.stack.push((subtree.clone(), index));
                                next_subtree = Some(child_trees[index].clone());
                                break;
                            }
                        }
                    }
                    Node::Leaf { items, .. } => {
                        let mut slice_items = SmallVec::<[T; 2 * TREE_BASE]>::new();
                        let mut slice_items_summary = T::Summary::default();

                        for (index, item) in items.iter().enumerate() {
                            let item_summary = item.summarize();
                            let child_end =
                                D::from_summary(&self.summary) + &D::from_summary(&item_summary);
                            if *pos > child_end || (*pos == child_end && bias == SeekBias::Right) {
                                if slice.is_some() {
                                    slice_items.push(item.clone());
                                    slice_items_summary += &item_summary;
                                }
                                self.summary += &item_summary;
                            } else {
                                if slice.is_some() {
                                    slice_subtrees.push(Tree::Resident(Arc::new(Node::Leaf {
                                        summary: slice_items_summary,
                                        items: slice_items,
                                    })));
                                }

                                self.stack.push((subtree.clone(), index));
                                break;
                            }
                        }
                    }
                };

                if let Some(next_subtree) = next_subtree {
                    subtree = next_subtree;
                } else {
                    break;
                }
            }
        }

        if let Some(slice) = slice.as_mut() {
            for subtree in slice_subtrees {
                slice.push_tree(subtree, db)?;
            }
        }

        Ok(())
    }
}

impl<T: Item> NullNodeStore<T> {
    fn new() -> Self {
        NullNodeStore(PhantomData)
    }
}

impl<T: Item> NodeStore<T> for NullNodeStore<T> {
    type ReadError = NullNodeStoreReadError;

    fn get(&mut self, _: NodeId) -> Result<&Node<T>, Self::ReadError> {
        Err(NullNodeStoreReadError)
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

fn sum_owned<T, I>(iter: I) -> T
where
    T: Default + for<'a> AddAssign<&'a T>,
    I: Iterator<Item = T>,
{
    let mut sum = T::default();
    for value in iter {
        sum += &value;
    }
    sum
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extend_and_push_tree() {
        let mut db = NullNodeStore::new();
        let db = &mut db;

        let mut tree1 = Tree::new();
        tree1.extend(0..20, db).unwrap();

        let mut tree2 = Tree::new();
        tree2.extend(50..100, db).unwrap();

        tree1.push_tree(tree2, db).unwrap();
        assert_eq!(
            tree1.items(db).unwrap(),
            (0..20).chain(50..100).collect::<Vec<u16>>()
        );
    }

    #[derive(Clone, Default, Debug)]
    pub struct IntegersSummary {
        count: usize,
        sum: usize,
    }

    impl Item for u16 {
        type Summary = IntegersSummary;

        fn summarize(&self) -> Self::Summary {
            IntegersSummary {
                count: 1,
                sum: *self as usize,
            }
        }
    }

    impl<'a> AddAssign<&'a Self> for IntegersSummary {
        fn add_assign(&mut self, other: &Self) {
            self.count += other.count;
            self.sum += other.sum;
        }
    }
}
