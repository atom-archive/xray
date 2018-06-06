use smallvec::SmallVec;
use std::cell::{Ref, RefCell};
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

    fn get(&self, id: NodeId) -> Result<Arc<Node<T>>, Self::ReadError>;
}

#[derive(Debug)]
pub enum Tree<T: Item> {
    Resident(Arc<Node<T>>),
    NonResident(NodeId),
}

#[derive(Debug)]
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

    pub fn cursor(&self) -> Cursor<T> {
        Cursor::new(self.clone())
    }

    pub fn last<S: NodeStore<T>>(&self, db: &S) -> Result<Option<T>, S::ReadError> {
        Ok(self.rightmost_leaf(db)?.node(db)?.items().last().cloned())
    }

    pub fn extent<D, S>(&self, db: &S) -> Result<D, S::ReadError>
    where
        S: NodeStore<T>,
        D: Dimension<Summary = T::Summary>,
    {
        match *self.node(db)? {
            Node::Internal { ref summary, .. } => Ok(D::from_summary(summary)),
            Node::Leaf { ref summary, .. } => Ok(D::from_summary(summary)),
        }
    }

    fn extend<I, S>(&mut self, iter: I, db: &S) -> Result<(), S::ReadError>
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

            let leaf = leaf.as_mut().unwrap();
            *leaf.summary_mut() += &item.summarize();
            leaf.items_mut().push(item);
        }

        if leaf.is_some() {
            self.push_tree(Tree::Resident(Arc::new(leaf.take().unwrap())), db)?;
        }

        Ok(())
    }

    pub fn push<S: NodeStore<T>>(&mut self, item: T, db: &S) -> Result<(), S::ReadError> {
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

    pub fn push_tree<S: NodeStore<T>>(&mut self, other: Self, db: &S) -> Result<(), S::ReadError> {
        let other_node = other.node(db)?;
        if self.node(db)?.height() < other_node.height() {
            for tree in other_node.child_trees() {
                self.push_tree(tree.clone(), db)?;
            }
        } else if let Some(split_tree) = self.push_tree_recursive(other, db)? {
            *self = Self::from_child_trees(vec![self.clone(), split_tree], db)?;
        }
        Ok(())
    }

    fn push_tree_recursive<S>(
        &mut self,
        other: Tree<T>,
        db: &S,
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
                let other_node = other.node(db)?;
                *summary += other_node.summary();

                let height_delta = *height - other_node.height();
                let mut summaries_to_append = SmallVec::<[T::Summary; 2 * TREE_BASE]>::new();
                let mut trees_to_append = SmallVec::<[Tree<T>; 2 * TREE_BASE]>::new();
                if height_delta == 0 {
                    summaries_to_append.extend(other_node.child_summaries().iter().cloned());
                    trees_to_append.extend(other_node.child_trees().iter().cloned());
                } else if height_delta == 1 && !other_node.is_underflowing() {
                    summaries_to_append.push(other_node.summary().clone());
                    trees_to_append.push(other)
                } else {
                    let tree_to_append = child_trees
                        .last_mut()
                        .unwrap()
                        .push_tree_recursive(other, db)?;
                    *child_summaries.last_mut().unwrap() = child_trees
                        .last()
                        .unwrap()
                        .node(db)
                        .unwrap()
                        .summary()
                        .clone();

                    if let Some(split_tree) = tree_to_append {
                        summaries_to_append.push(split_tree.node(db).unwrap().summary().clone());
                        trees_to_append.push(split_tree);
                    }
                }

                let child_count = child_trees.len() + trees_to_append.len();
                if child_count > 2 * TREE_BASE {
                    let left_summaries: SmallVec<_>;
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
                    *summary = sum(left_summaries.iter());
                    *child_summaries = left_summaries;
                    *child_trees = left_trees;

                    Ok(Some(Tree::Resident(Arc::new(Node::Internal {
                        height: *height,
                        summary: sum(right_summaries.iter()),
                        child_summaries: right_summaries,
                        child_trees: right_trees,
                    }))))
                } else {
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

    fn from_child_trees<S>(child_trees: Vec<Tree<T>>, db: &S) -> Result<Self, S::ReadError>
    where
        S: NodeStore<T>,
    {
        let height = child_trees[0].node(db)?.height() + 1;
        let mut child_summaries = SmallVec::new();
        for child in &child_trees {
            child_summaries.push(child.node(db)?.summary().clone());
        }
        let summary = sum(child_summaries.iter());
        Ok(Tree::Resident(Arc::new(Node::Internal {
            height,
            summary,
            child_summaries,
            child_trees: SmallVec::from_vec(child_trees),
        })))
    }

    fn make_mut_node<S: NodeStore<T>>(&mut self, db: &S) -> Result<&mut Node<T>, S::ReadError> {
        if let Tree::NonResident(node_id) = *self {
            *self = Tree::Resident(db.get(node_id)?);
        }

        match self {
            Tree::Resident(node) => Ok(Arc::make_mut(node)),
            Tree::NonResident(_) => unreachable!(),
        }
    }

    fn node<S: NodeStore<T>>(&self, db: &S) -> Result<Arc<Node<T>>, S::ReadError> {
        match self {
            Tree::Resident(node) => Ok(node.clone()),
            Tree::NonResident(node_id) => db.get(*node_id),
        }
    }

    fn rightmost_leaf<S: NodeStore<T>>(&self, db: &S) -> Result<Tree<T>, S::ReadError> {
        match *self.node(db)? {
            Node::Leaf { .. } => Ok(self.clone()),
            Node::Internal {
                ref child_trees, ..
            } => child_trees.last().unwrap().rightmost_leaf(db),
        }
    }
}

impl<T: Item> Clone for Tree<T> {
    fn clone(&self) -> Self {
        match self {
            Tree::Resident(node) => Tree::Resident(node.clone()),
            Tree::NonResident(id) => Tree::NonResident(*id),
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

    fn summary_mut(&mut self) -> &mut T::Summary {
        match self {
            Node::Internal { summary, .. } => summary,
            Node::Leaf { summary, .. } => summary,
        }
    }

    fn is_underflowing(&self) -> bool {
        match self {
            Node::Internal { child_trees, .. } => child_trees.len() < TREE_BASE,
            Node::Leaf { items, .. } => items.len() < TREE_BASE,
        }
    }
}

impl<T: Item> Clone for Node<T> {
    fn clone(&self) -> Self {
        match self {
            Node::Internal {
                height,
                summary,
                child_summaries,
                child_trees,
            } => Node::Internal {
                height: *height,
                summary: summary.clone(),
                child_summaries: child_summaries.clone(),
                child_trees: child_trees.clone(),
            },
            Node::Leaf { summary, items } => Node::Leaf {
                summary: summary.clone(),
                items: items.clone(),
            },
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

    pub fn item<S: NodeStore<T>>(&self, db: &S) -> Result<Option<T>, S::ReadError> {
        assert!(self.did_seek, "Must seek before calling this method");
        if let Some((subtree, index)) = self.stack.last() {
            match *subtree.node(db)? {
                Node::Leaf { ref items, .. } => {
                    if *index == items.len() {
                        Ok(None)
                    } else {
                        Ok(Some(items[*index].clone()))
                    }
                }
                _ => unreachable!(),
            }
        } else {
            Ok(None)
        }
    }

    pub fn prev_item<S: NodeStore<T>>(&self, db: &S) -> Result<Option<T>, S::ReadError> {
        assert!(self.did_seek, "Must seek before calling this method");
        if let Some((cur_leaf, index)) = self.stack.last() {
            if *index == 0 {
                if let Some(prev_leaf) = self.prev_leaf(db)? {
                    let prev_leaf = prev_leaf.node(db)?;
                    Ok(Some(prev_leaf.items().last().unwrap().clone()))
                } else {
                    Ok(None)
                }
            } else {
                match *cur_leaf.node(db)? {
                    Node::Leaf { ref items, .. } => Ok(Some(items[index - 1].clone())),
                    _ => unreachable!(),
                }
            }
        } else {
            self.tree.last(db)
        }
    }

    fn prev_leaf<S: NodeStore<T>>(&self, db: &S) -> Result<Option<Tree<T>>, S::ReadError> {
        for (ancestor, index) in self.stack.iter().rev().skip(1) {
            if *index != 0 {
                match *ancestor.node(db)? {
                    Node::Internal {
                        ref child_trees, ..
                    } => return Ok(Some(child_trees[index - 1].rightmost_leaf(db)?)),
                    Node::Leaf { .. } => unreachable!(),
                };
            }
        }
        Ok(None)
    }

    pub fn next<S: NodeStore<T>>(&mut self, db: &S) -> Result<(), S::ReadError> {
        assert!(self.did_seek, "Must seek before calling this method");

        while self.stack.len() > 0 {
            let new_subtree = {
                let (subtree, index) = self.stack.last_mut().unwrap();
                match *subtree.node(db)? {
                    Node::Internal {
                        ref child_trees, ..
                    } => {
                        *index += 1;
                        child_trees.get(*index).cloned()
                    }
                    Node::Leaf { ref items, .. } => {
                        self.summary += &items[*index].summarize();
                        *index += 1;
                        if *index < items.len() {
                            return Ok(());
                        } else {
                            None
                        }
                    }
                }
            };

            if let Some(subtree) = new_subtree {
                self.descend_to_start(subtree, db)?;
                break;
            } else {
                self.stack.pop();
            }
        }

        Ok(())
    }

    fn descend_to_start<S>(&mut self, mut subtree: Tree<T>, db: &S) -> Result<(), S::ReadError>
    where
        S: NodeStore<T>,
    {
        self.did_seek = true;
        loop {
            self.stack.push((subtree.clone(), 0));
            subtree = match *subtree.node(db)? {
                Node::Internal {
                    ref child_trees, ..
                } => child_trees[0].clone(),
                Node::Leaf { .. } => {
                    return Ok(());
                }
            }
        }
    }

    pub fn seek<D, S>(&mut self, pos: &D, bias: SeekBias, db: &S) -> Result<(), S::ReadError>
    where
        D: Dimension<Summary = T::Summary>,
        S: NodeStore<T>,
    {
        self.reset();
        self.seek_internal(pos, bias, db, None)
    }

    pub fn slice<D, S>(&mut self, end: &D, bias: SeekBias, db: &S) -> Result<Tree<T>, S::ReadError>
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
        db: &S,
        mut slice: Option<&mut Tree<T>>,
    ) -> Result<(), S::ReadError>
    where
        D: Dimension<Summary = T::Summary>,
        S: NodeStore<T>,
    {
        let mut containing_subtree = None;

        if self.did_seek {
            'outer: while self.stack.len() > 0 {
                {
                    let (parent_subtree, index) = self.stack.last_mut().unwrap();
                    match *parent_subtree.node(db)? {
                        Node::Internal {
                            ref child_summaries,
                            ref child_trees,
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
                                    slice.as_mut().unwrap().push_tree(child_tree.clone(), db)?;
                                    *index += 1;
                                } else {
                                    containing_subtree = Some(child_tree.clone());
                                }
                            }
                        }
                        Node::Leaf { ref items, .. } => {
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
                                    slice.as_mut().unwrap().push_tree(
                                        Tree::Resident(Arc::new(Node::Leaf {
                                            summary: slice_items_summary,
                                            items: slice_items,
                                        })),
                                        db,
                                    )?;
                                    break 'outer;
                                }
                            }

                            slice.as_mut().unwrap().push_tree(
                                Tree::Resident(Arc::new(Node::Leaf {
                                    summary: slice_items_summary,
                                    items: slice_items,
                                })),
                                db,
                            )?;
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
            self.did_seek = true;
            containing_subtree = Some(self.tree.clone());
        }

        if let Some(mut subtree) = containing_subtree {
            loop {
                let mut next_subtree = None;
                match *subtree.node(db)? {
                    Node::Internal {
                        ref child_summaries,
                        ref child_trees,
                        ..
                    } => {
                        for (index, child_summary) in child_summaries.iter().enumerate() {
                            let child_end =
                                D::from_summary(&self.summary) + &D::from_summary(child_summary);
                            if *pos > child_end || (*pos == child_end && bias == SeekBias::Right) {
                                self.summary += child_summary;
                                if let Some(slice) = slice.as_mut() {
                                    slice.push_tree(child_trees[index].clone(), db)?;
                                }
                            } else {
                                self.stack.push((subtree.clone(), index));
                                next_subtree = Some(child_trees[index].clone());
                                break;
                            }
                        }
                    }
                    Node::Leaf { ref items, .. } => {
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
                                self.stack.push((subtree.clone(), index));
                                break;
                            }
                        }

                        if let Some(slice) = slice.as_mut() {
                            if slice_items.len() > 0 {
                                slice.push_tree(
                                    Tree::Resident(Arc::new(Node::Leaf {
                                        summary: slice_items_summary,
                                        items: slice_items,
                                    })),
                                    db,
                                )?;
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

    fn get(&self, _: NodeId) -> Result<Arc<Node<T>>, Self::ReadError> {
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
    extern crate rand;

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
            (0..20).chain(50..100).collect::<Vec<u8>>()
        );
    }

    #[test]
    fn test_random() {
        for seed in 0..100 {
            use self::rand::{Rng, SeedableRng, StdRng};

            let mut rng = StdRng::from_seed(&[seed]);

            let mut db = NullNodeStore::new();
            let db = &mut db;
            let mut tree = Tree::<u8>::new();
            let count = rng.gen_range(0, 10);
            tree.extend(rng.gen_iter().take(count), db).unwrap();

            for _i in 0..10 {
                let splice_end = rng.gen_range(0, tree.extent::<Count, _>(db).unwrap().0 + 1);
                let splice_start = rng.gen_range(0, splice_end + 1);
                let count = rng.gen_range(0, 3);
                let tree_end = tree.extent::<Count, _>(db).unwrap();
                let new_items = rng.gen_iter().take(count).collect::<Vec<u8>>();

                let mut reference_items = tree.items(db).unwrap();
                reference_items.splice(splice_start..splice_end, new_items.clone());

                let mut cursor = tree.cursor();
                tree = cursor
                    .slice(&Count(splice_start), SeekBias::Right, db)
                    .unwrap();
                tree.extend(new_items, db).unwrap();
                cursor
                    .seek(&Count(splice_end), SeekBias::Right, db)
                    .unwrap();
                tree.push_tree(cursor.slice(&tree_end, SeekBias::Right, db).unwrap(), db)
                    .unwrap();

                assert_eq!(tree.items(db).unwrap(), reference_items);

                let mut pos = rng.gen_range(0, tree.extent::<Count, _>(db).unwrap().0 + 1);
                let mut cursor = tree.cursor();
                cursor.seek(&Count(pos), SeekBias::Right, db).unwrap();

                for _i in 0..5 {
                    if pos > 0 {
                        assert_eq!(
                            *cursor.prev_item(db).unwrap().unwrap(),
                            reference_items[pos - 1]
                        );
                    } else {
                        assert_eq!(cursor.prev_item(db).unwrap(), None);
                    }

                    if pos < reference_items.len() {
                        assert_eq!(*cursor.item(db).unwrap().unwrap(), reference_items[pos]);
                    } else {
                        assert_eq!(cursor.item(db).unwrap(), None);
                    }

                    cursor.next(db);
                    if pos < reference_items.len() {
                        pos += 1;
                    }
                }
            }
        }
    }

    #[derive(Clone, Default, Debug)]
    pub struct IntegersSummary {
        count: usize,
        sum: usize,
    }

    #[derive(Ord, PartialOrd, Default, Eq, PartialEq, Clone, Debug)]
    struct Count(usize);

    impl Item for u8 {
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

    impl<T: Clone + Item> Tree<T> {
        pub fn items<S: NodeStore<T>>(&self, db: &S) -> Result<Vec<T>, S::ReadError> {
            let mut items = Vec::new();
            let mut cursor = self.cursor();
            cursor.descend_to_start(self.clone(), db)?;
            loop {
                if let Some(item) = cursor.item(db)? {
                    items.push(item.as_ref().clone());
                } else {
                    break;
                }
                cursor.next(db)?;
            }
            Ok(items)
        }
    }
}
