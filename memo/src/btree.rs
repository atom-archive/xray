use futures::future::{self, ExecuteError, Executor};
use futures::sync::oneshot;
use futures::Future;
use parking_lot::Mutex;
use smallvec::SmallVec;
use std::cmp::Ordering;
use std::fmt;
use std::ops::{Add, AddAssign};
use std::sync::Arc;

const TREE_BASE: usize = 16;
pub type NodeId = usize;

pub trait Item: Clone + Eq + fmt::Debug {
    type Summary: for<'a> AddAssign<&'a Self::Summary> + Default + Clone + fmt::Debug;

    fn summarize(&self) -> Self::Summary;
}

pub trait KeyedItem: Item {
    type Key: Dimension<Self::Summary>;

    fn key(&self) -> Self::Key;
}

pub trait Dimension<Summary: Default>:
    for<'a> Add<&'a Self, Output = Self> + for<'a> AddAssign<&'a Self> + Ord + Clone + fmt::Debug
{
    fn from_summary(summary: &Summary) -> Self;

    fn default() -> Self {
        Self::from_summary(&Summary::default()).clone()
    }
}

pub trait Store<T: Item>: Clone {
    type WriteError;

    fn read(&self, id: NodeId) -> Node<T>;
    fn write(&self, node: &Node<T>) -> Result<NodeId, Self::WriteError>;
}

#[derive(Debug)]
pub enum Tree<T: Item> {
    Resident(Arc<Node<T>>),
    NonResident(NodeId),
}

#[derive(Debug)]
pub enum Node<T: Item> {
    Internal {
        store_id: Mutex<Option<NodeId>>,
        height: u8,
        summary: T::Summary,
        child_summaries: SmallVec<[T::Summary; 2 * TREE_BASE]>,
        child_trees: SmallVec<[Tree<T>; 2 * TREE_BASE]>,
    },
    Leaf {
        store_id: Mutex<Option<NodeId>>,
        summary: T::Summary,
        items: SmallVec<[T; 2 * TREE_BASE]>,
    },
}

#[derive(Clone)]
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
pub enum Edit<T: KeyedItem> {
    Insert(T),
    Remove(T),
}

pub struct SaveResult<
    W,
    B: Future<Item = (), Error = ()>,
    F: Future<Item = (), Error = SaveError<W>>,
> {
    background: B,
    foreground: F,
}

pub enum SaveError<W> {
    WriteError(W),
    Canceled,
}

impl<T: Item> Tree<T> {
    pub fn new() -> Self {
        Tree::Resident(Arc::new(Node::Leaf {
            store_id: Mutex::new(None),
            summary: T::Summary::default(),
            items: SmallVec::new(),
        }))
    }

    pub fn save<S>(
        &self,
        store: S,
    ) -> SaveResult<
        S::WriteError,
        impl Future<Item = (), Error = ()>,
        impl Future<Item = (), Error = SaveError<S::WriteError>>,
    >
    where
        S: 'static + Store<T>,
    {
        let snapshot = self.clone();
        let (tx, rx) = oneshot::channel();
        SaveResult {
            background: future::lazy(move || {
                let _ = tx.send(snapshot.save_internal(store));
                Ok(())
            }),
            foreground: rx
                .map_err(|_| SaveError::Canceled)
                .and_then(|result| result.map_err(|err| SaveError::WriteError(err))),
        }
    }

    pub fn save_internal<S>(&self, store: S) -> Result<(), S::WriteError>
    where
        S: Store<T>,
    {
        match self {
            Tree::Resident(node) => match node.as_ref() {
                Node::Internal {
                    store_id,
                    child_trees,
                    ..
                } => {
                    let mut store_id = store_id.lock();
                    if store_id.is_none() {
                        for tree in child_trees {
                            tree.save_internal(store.clone())?;
                        }
                        *store_id = Some(store.write(node)?)
                    }
                }
                Node::Leaf { store_id, .. } => {
                    let mut store_id = store_id.lock();
                    if store_id.is_none() {
                        *store_id = Some(store.write(node)?)
                    }
                }
            },
            _ => {}
        }

        Ok(())
    }

    pub fn items(&self) -> Vec<T> {
        let mut items = Vec::new();
        let mut cursor = self.cursor();
        cursor.descend_to_start(self.clone());
        loop {
            if let Some(item) = cursor.item() {
                items.push(item);
            } else {
                break;
            }
            cursor.next();
        }
        items
    }

    pub fn cursor(&self) -> Cursor<T> {
        Cursor::new(self.clone())
    }

    pub fn first(&self) -> Option<T> {
        self.leftmost_leaf().node().items().first().cloned()
    }

    pub fn last(&self) -> Option<T> {
        self.rightmost_leaf().node().items().last().cloned()
    }

    pub fn extent<D: Dimension<T::Summary>>(&self) -> D {
        match *self.node() {
            Node::Internal { ref summary, .. } => D::from_summary(summary).clone(),
            Node::Leaf { ref summary, .. } => D::from_summary(summary).clone(),
        }
    }

    pub fn insert<D>(&mut self, position: &D, bias: SeekBias, item: T)
    where
        D: Dimension<T::Summary>,
    {
        let mut cursor = self.cursor();
        let mut new_tree = cursor.slice(position, bias);
        new_tree.push(item);
        let suffix = cursor.slice(&self.extent::<D>(), SeekBias::Right);
        new_tree.push_tree(suffix);
        *self = new_tree;
    }

    pub fn extend<I>(&mut self, iter: I)
    where
        I: IntoIterator<Item = T>,
    {
        let mut leaf: Option<Node<T>> = None;

        for item in iter {
            if leaf.is_some() && leaf.as_ref().unwrap().items().len() == 2 * TREE_BASE {
                self.push_tree(Tree::Resident(Arc::new(leaf.take().unwrap())));
            }

            if leaf.is_none() {
                leaf = Some(Node::Leaf::<T> {
                    store_id: Mutex::new(None),
                    summary: T::Summary::default(),
                    items: SmallVec::new(),
                });
            }

            let leaf = leaf.as_mut().unwrap();
            *leaf.summary_mut() += &item.summarize();
            leaf.items_mut().push(item);
        }

        if leaf.is_some() {
            self.push_tree(Tree::Resident(Arc::new(leaf.take().unwrap())));
        }
    }

    pub fn push(&mut self, item: T) {
        self.push_tree(Tree::from_child_trees(vec![Tree::Resident(Arc::new(
            Node::Leaf {
                store_id: Mutex::new(None),
                summary: item.summarize(),
                items: SmallVec::from_vec(vec![item]),
            },
        ))]))
    }

    pub fn push_tree(&mut self, other: Self) {
        let other_node = other.node();
        if !other_node.is_leaf() || other_node.items().len() > 0 {
            if self.node().height() < other_node.height() {
                for tree in other_node.child_trees() {
                    self.push_tree(tree.clone());
                }
            } else if let Some(split_tree) = self.push_tree_recursive(other) {
                *self = Self::from_child_trees(vec![self.clone(), split_tree]);
            }
        }
    }

    fn push_tree_recursive(&mut self, other: Tree<T>) -> Option<Tree<T>> {
        match self.make_mut_node() {
            Node::Internal {
                height,
                summary,
                child_summaries,
                child_trees,
                ..
            } => {
                let other_node = other.node();
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
                    let tree_to_append = child_trees.last_mut().unwrap().push_tree_recursive(other);
                    *child_summaries.last_mut().unwrap() =
                        child_trees.last().unwrap().node().summary().clone();

                    if let Some(split_tree) = tree_to_append {
                        summaries_to_append.push(split_tree.node().summary().clone());
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

                    Some(Tree::Resident(Arc::new(Node::Internal {
                        store_id: Mutex::new(None),
                        height: *height,
                        summary: sum(right_summaries.iter()),
                        child_summaries: right_summaries,
                        child_trees: right_trees,
                    })))
                } else {
                    child_summaries.extend(summaries_to_append);
                    child_trees.extend(trees_to_append);
                    None
                }
            }
            Node::Leaf { summary, items, .. } => {
                let other_node = other.node();

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
                    Some(Tree::Resident(Arc::new(Node::Leaf {
                        store_id: Mutex::new(None),
                        summary: sum_owned(right_items.iter().map(|item| item.summarize())),
                        items: right_items,
                    })))
                } else {
                    *summary += other_node.summary();
                    items.extend(other_node.items().iter().cloned());
                    None
                }
            }
        }
    }

    fn from_child_trees(child_trees: Vec<Tree<T>>) -> Self {
        let height = child_trees[0].node().height() + 1;
        let mut child_summaries = SmallVec::new();
        for child in &child_trees {
            child_summaries.push(child.node().summary().clone());
        }
        let summary = sum(child_summaries.iter());
        Tree::Resident(Arc::new(Node::Internal {
            store_id: Mutex::new(None),
            height,
            summary,
            child_summaries,
            child_trees: SmallVec::from_vec(child_trees),
        }))
    }

    fn make_mut_node(&mut self) -> &mut Node<T> {
        match self {
            Tree::Resident(node) => Arc::make_mut(node),
            Tree::NonResident(_) => panic!(),
        }
    }

    fn node(&self) -> Arc<Node<T>> {
        match self {
            Tree::Resident(node) => node.clone(),
            Tree::NonResident(_) => panic!(),
        }
    }

    fn leftmost_leaf(&self) -> Tree<T> {
        match *self.node() {
            Node::Leaf { .. } => self.clone(),
            Node::Internal {
                ref child_trees, ..
            } => child_trees.first().unwrap().leftmost_leaf(),
        }
    }

    fn rightmost_leaf(&self) -> Tree<T> {
        match *self.node() {
            Node::Leaf { .. } => self.clone(),
            Node::Internal {
                ref child_trees, ..
            } => child_trees.last().unwrap().rightmost_leaf(),
        }
    }
}

impl<T: KeyedItem> Tree<T> {
    pub fn edit(&mut self, mut edits: Vec<Edit<T>>) {
        edits.sort_unstable_by_key(|item| item.key());

        let mut cursor = self.cursor();
        let mut new_tree = Tree::new();
        let mut buffered_items = Vec::new();

        cursor.seek(&T::Key::default(), SeekBias::Left);
        for edit in edits {
            let new_key = edit.key();
            let mut old_item = cursor.item();

            if old_item
                .as_ref()
                .map_or(false, |old_item| old_item.key() < new_key)
            {
                new_tree.extend(buffered_items.drain(..));
                let slice = cursor.slice(&new_key, SeekBias::Left);
                new_tree.push_tree(slice);
                old_item = cursor.item();
            }
            if old_item.map_or(false, |old_item| old_item.key() == new_key) {
                cursor.next();
            }
            match edit {
                Edit::Insert(item) => {
                    buffered_items.push(item);
                }
                Edit::Remove(_) => {}
            }
        }

        new_tree.extend(buffered_items);
        new_tree.push_tree(cursor.suffix::<T::Key>());

        *self = new_tree;
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
    fn is_leaf(&self) -> bool {
        match self {
            Node::Leaf { .. } => true,
            _ => false,
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

impl<T: Item> Node<T> {
    fn store_id(&self) -> &Mutex<Option<NodeId>> {
        match self {
            Node::Internal { store_id, .. } => store_id,
            Node::Leaf { store_id, .. } => store_id,
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
                ..
            } => Node::Internal {
                store_id: Mutex::new(None),
                height: *height,
                summary: summary.clone(),
                child_summaries: child_summaries.clone(),
                child_trees: child_trees.clone(),
            },
            Node::Leaf { summary, items, .. } => Node::Leaf {
                store_id: Mutex::new(None),
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

    pub fn start<D: Dimension<T::Summary>>(&self) -> D {
        D::from_summary(&self.summary).clone()
    }

    pub fn end<D: Dimension<T::Summary>>(&self) -> D {
        if let Some(item) = self.item() {
            self.start::<D>() + &D::from_summary(&item.summarize())
        } else {
            self.start::<D>()
        }
    }

    pub fn item(&self) -> Option<T> {
        assert!(self.did_seek, "Must seek before calling this method");
        if let Some((subtree, index)) = self.stack.last() {
            match *subtree.node() {
                Node::Leaf { ref items, .. } => {
                    if *index == items.len() {
                        None
                    } else {
                        Some(items[*index].clone())
                    }
                }
                _ => unreachable!(),
            }
        } else {
            None
        }
    }

    pub fn prev_item(&self) -> Option<T> {
        assert!(self.did_seek, "Must seek before calling this method");
        if let Some((cur_leaf, index)) = self.stack.last() {
            if *index == 0 {
                if let Some(prev_leaf) = self.prev_leaf() {
                    let prev_leaf = prev_leaf.node();
                    Some(prev_leaf.items().last().unwrap().clone())
                } else {
                    None
                }
            } else {
                match *cur_leaf.node() {
                    Node::Leaf { ref items, .. } => Some(items[index - 1].clone()),
                    _ => unreachable!(),
                }
            }
        } else {
            self.tree.last()
        }
    }

    fn prev_leaf(&self) -> Option<Tree<T>> {
        for (ancestor, index) in self.stack.iter().rev().skip(1) {
            if *index != 0 {
                match *ancestor.node() {
                    Node::Internal {
                        ref child_trees, ..
                    } => return Some(child_trees[index - 1].rightmost_leaf()),
                    Node::Leaf { .. } => unreachable!(),
                };
            }
        }
        None
    }

    pub fn next(&mut self) {
        assert!(self.did_seek, "Must seek before calling this method");

        while self.stack.len() > 0 {
            let new_subtree = {
                let (subtree, index) = self.stack.last_mut().unwrap();
                match *subtree.node() {
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
                            return;
                        } else {
                            None
                        }
                    }
                }
            };

            if let Some(subtree) = new_subtree {
                self.descend_to_start(subtree);
                break;
            } else {
                self.stack.pop();
            }
        }
    }

    fn descend_to_start(&mut self, mut subtree: Tree<T>) {
        self.did_seek = true;
        loop {
            self.stack.push((subtree.clone(), 0));
            subtree = match *subtree.node() {
                Node::Internal {
                    ref child_trees, ..
                } => child_trees[0].clone(),
                Node::Leaf { .. } => {
                    break;
                }
            }
        }
    }

    pub fn seek<D>(&mut self, pos: &D, bias: SeekBias) -> bool
    where
        D: Dimension<T::Summary>,
    {
        self.reset();
        self.seek_internal(pos, bias, None)
    }

    pub fn seek_forward<D>(&mut self, pos: &D, bias: SeekBias) -> bool
    where
        D: Dimension<T::Summary>,
    {
        self.seek_internal(pos, bias, None)
    }

    pub fn slice<D>(&mut self, end: &D, bias: SeekBias) -> Tree<T>
    where
        D: Dimension<T::Summary>,
    {
        let mut slice = Tree::new();
        self.seek_internal(end, bias, Some(&mut slice));
        slice
    }

    pub fn suffix<D>(&mut self) -> Tree<T>
    where
        D: Dimension<T::Summary>,
    {
        let extent = self.tree.extent::<D>();
        let mut slice = Tree::new();
        self.seek_internal(&extent, SeekBias::Right, Some(&mut slice));
        slice
    }

    fn seek_internal<D>(
        &mut self,
        target: &D,
        bias: SeekBias,
        mut slice: Option<&mut Tree<T>>,
    ) -> bool
    where
        D: Dimension<T::Summary>,
    {
        let mut pos = D::from_summary(&self.summary).clone();
        debug_assert!(target >= &pos);
        let mut containing_subtree = None;

        if self.did_seek {
            'outer: while self.stack.len() > 0 {
                {
                    let (parent_subtree, index) = self.stack.last_mut().unwrap();
                    match *parent_subtree.node() {
                        Node::Internal {
                            ref child_summaries,
                            ref child_trees,
                            ..
                        } => {
                            *index += 1;
                            while *index < child_summaries.len() {
                                let child_tree = &child_trees[*index];
                                let child_summary = &child_summaries[*index];
                                let mut child_end = pos;
                                child_end += &D::from_summary(&child_summary);

                                let comparison = target.cmp(&child_end);
                                if comparison == Ordering::Greater
                                    || (comparison == Ordering::Equal && bias == SeekBias::Right)
                                {
                                    self.summary += child_summary;
                                    pos = child_end;
                                    if let Some(slice) = slice.as_mut() {
                                        slice.push_tree(child_tree.clone());
                                    }
                                    *index += 1;
                                } else {
                                    pos = D::from_summary(&self.summary).clone();
                                    containing_subtree = Some(child_tree.clone());
                                    break 'outer;
                                }
                            }
                        }
                        Node::Leaf { ref items, .. } => {
                            let mut slice_items = SmallVec::<[T; 2 * TREE_BASE]>::new();
                            let mut slice_items_summary = T::Summary::default();

                            while *index < items.len() {
                                let item = &items[*index];
                                let item_summary = item.summarize();
                                let mut item_end = pos;
                                item_end += &D::from_summary(&item_summary);

                                let comparison = target.cmp(&item_end);
                                if comparison == Ordering::Greater
                                    || (comparison == Ordering::Equal && bias == SeekBias::Right)
                                {
                                    self.summary += &item_summary;
                                    pos = item_end;
                                    if slice.is_some() {
                                        slice_items.push(item.clone());
                                        slice_items_summary += &item_summary;
                                    }
                                    *index += 1;
                                } else {
                                    pos = D::from_summary(&self.summary).clone();
                                    if let Some(slice) = slice.as_mut() {
                                        slice.push_tree(Tree::Resident(Arc::new(Node::Leaf {
                                            store_id: Mutex::new(None),
                                            summary: slice_items_summary,
                                            items: slice_items,
                                        })));
                                    }
                                    break 'outer;
                                }
                            }

                            if let Some(slice) = slice.as_mut() {
                                if slice_items.len() > 0 {
                                    slice.push_tree(Tree::Resident(Arc::new(Node::Leaf {
                                        store_id: Mutex::new(None),
                                        summary: slice_items_summary,
                                        items: slice_items,
                                    })));
                                }
                            }
                        }
                    }
                }

                self.stack.pop();
            }
        } else {
            self.did_seek = true;
            containing_subtree = Some(self.tree.clone());
        }

        if let Some(mut subtree) = containing_subtree {
            loop {
                let mut next_subtree = None;
                match *subtree.node() {
                    Node::Internal {
                        ref child_summaries,
                        ref child_trees,
                        ..
                    } => {
                        for (index, child_summary) in child_summaries.iter().enumerate() {
                            let mut child_end = pos;
                            child_end += &D::from_summary(child_summary);

                            let comparison = target.cmp(&child_end);
                            if comparison == Ordering::Greater
                                || (comparison == Ordering::Equal && bias == SeekBias::Right)
                            {
                                self.summary += child_summary;
                                pos = child_end;
                                if let Some(slice) = slice.as_mut() {
                                    slice.push_tree(child_trees[index].clone());
                                }
                            } else {
                                pos = D::from_summary(&self.summary).clone();
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
                            let mut child_end = pos;
                            child_end += &D::from_summary(&item_summary);

                            let comparison = target.cmp(&child_end);
                            if comparison == Ordering::Greater
                                || (comparison == Ordering::Equal && bias == SeekBias::Right)
                            {
                                if slice.is_some() {
                                    slice_items.push(item.clone());
                                    slice_items_summary += &item_summary;
                                }
                                self.summary += &item_summary;
                                pos = child_end;
                            } else {
                                pos = D::from_summary(&self.summary).clone();
                                self.stack.push((subtree.clone(), index));
                                break;
                            }
                        }

                        if let Some(slice) = slice.as_mut() {
                            if slice_items.len() > 0 {
                                slice.push_tree(Tree::Resident(Arc::new(Node::Leaf {
                                    store_id: Mutex::new(None),
                                    summary: slice_items_summary,
                                    items: slice_items,
                                })));
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

        if bias == SeekBias::Left {
            *target == self.end::<D>()
        } else {
            *target == self.start::<D>()
        }
    }
}

impl<T: KeyedItem> Edit<T> {
    fn key(&self) -> T::Key {
        match self {
            Edit::Insert(item) | Edit::Remove(item) => item.key(),
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
        let mut tree1 = Tree::new();
        tree1.extend(0..20);

        let mut tree2 = Tree::new();
        tree2.extend(50..100);

        tree1.push_tree(tree2);
        assert_eq!(tree1.items(), (0..20).chain(50..100).collect::<Vec<u8>>());
    }

    #[test]
    fn test_random() {
        for seed in 0..10000 {
            use self::rand::{Rng, SeedableRng, StdRng};

            let mut rng = StdRng::from_seed(&[seed]);

            let mut tree = Tree::<u8>::new();
            let count = rng.gen_range(0, 10);
            tree.extend(rng.gen_iter().take(count));

            for _i in 0..10 {
                let splice_end = rng.gen_range(0, tree.extent::<Count>().0 + 1);
                let splice_start = rng.gen_range(0, splice_end + 1);
                let count = rng.gen_range(0, 3);
                let tree_end = tree.extent::<Count>();
                let new_items = rng.gen_iter().take(count).collect::<Vec<u8>>();

                let mut reference_items = tree.items();
                reference_items.splice(splice_start..splice_end, new_items.clone());

                let mut cursor = tree.cursor();
                tree = cursor.slice(&Count(splice_start), SeekBias::Right);
                tree.extend(new_items);
                cursor.seek(&Count(splice_end), SeekBias::Right);
                tree.push_tree(cursor.slice(&tree_end, SeekBias::Right));

                assert_eq!(tree.items(), reference_items);

                let mut pos = rng.gen_range(0, tree.extent::<Count>().0 + 1);
                let mut cursor = tree.cursor();
                cursor.seek(&Count(pos), SeekBias::Right);

                for _i in 0..5 {
                    if pos > 0 {
                        assert_eq!(cursor.prev_item().unwrap(), reference_items[pos - 1]);
                    } else {
                        assert_eq!(cursor.prev_item(), None);
                    }

                    if pos < reference_items.len() {
                        assert_eq!(cursor.item().unwrap(), reference_items[pos]);
                    } else {
                        assert_eq!(cursor.item(), None);
                    }

                    cursor.next();
                    if pos < reference_items.len() {
                        pos += 1;
                    }
                }
            }
        }
    }

    #[derive(Clone, Default, Debug)]
    pub struct IntegersSummary {
        count: Count,
    }

    #[derive(Ord, PartialOrd, Default, Eq, PartialEq, Clone, Debug)]
    struct Count(usize);

    impl Item for u8 {
        type Summary = IntegersSummary;

        fn summarize(&self) -> Self::Summary {
            IntegersSummary { count: Count(1) }
        }
    }

    impl<'a> AddAssign<&'a Self> for IntegersSummary {
        fn add_assign(&mut self, other: &Self) {
            self.count += &other.count;
        }
    }

    impl Dimension<IntegersSummary> for Count {
        fn from_summary(summary: &IntegersSummary) -> Self {
            summary.count.clone()
        }
    }

    impl<'a> AddAssign<&'a Self> for Count {
        fn add_assign(&mut self, other: &Self) {
            self.0 += other.0;
        }
    }

    impl<'a> Add<&'a Self> for Count {
        type Output = Self;

        fn add(mut self, other: &Self) -> Self {
            self.0 += other.0;
            self
        }
    }
}
