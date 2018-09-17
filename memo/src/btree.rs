use smallvec::SmallVec;
use std::cmp::Ordering;
use std::fmt;
use std::ops::{Add, AddAssign};
use std::sync::Arc;

const TREE_BASE: usize = 16;

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

#[derive(Debug, Clone)]
pub struct Tree<T: Item>(Arc<Node<T>>);

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

#[derive(Clone)]
pub struct Cursor<T: Item> {
    tree: Tree<T>,
    stack: SmallVec<[(Tree<T>, usize, T::Summary); 16]>,
    summary: T::Summary,
    did_seek: bool,
    at_end: bool,
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

impl<T: Item> Tree<T> {
    pub fn new() -> Self {
        Tree(Arc::new(Node::Leaf {
            summary: T::Summary::default(),
            items: SmallVec::new(),
        }))
    }

    pub fn from_item(item: T) -> Self {
        let mut tree = Self::new();
        tree.push(item);
        tree
    }

    #[cfg(test)]
    pub fn items(&self) -> Vec<T> {
        let mut items = Vec::new();
        let mut cursor = self.cursor();
        cursor.descend_to_first_item(self.clone());
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

    #[allow(dead_code)]
    pub fn first(&self) -> Option<T> {
        self.leftmost_leaf().0.items().first().cloned()
    }

    pub fn last(&self) -> Option<T> {
        self.rightmost_leaf().0.items().last().cloned()
    }

    pub fn extent<D: Dimension<T::Summary>>(&self) -> D {
        match self.0.as_ref() {
            Node::Internal { summary, .. } => D::from_summary(summary).clone(),
            Node::Leaf { summary, .. } => D::from_summary(summary).clone(),
        }
    }

    pub fn summary(&self) -> T::Summary {
        match self.0.as_ref() {
            Node::Internal { summary, .. } => summary.clone(),
            Node::Leaf { summary, .. } => summary.clone(),
        }
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        match self.0.as_ref() {
            Node::Internal { .. } => false,
            Node::Leaf { items, .. } => items.is_empty(),
        }
    }

    pub fn extend<I>(&mut self, iter: I)
    where
        I: IntoIterator<Item = T>,
    {
        let mut leaf: Option<Node<T>> = None;

        for item in iter {
            if leaf.is_some() && leaf.as_ref().unwrap().items().len() == 2 * TREE_BASE {
                self.push_tree(Tree(Arc::new(leaf.take().unwrap())));
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
            self.push_tree(Tree(Arc::new(leaf.take().unwrap())));
        }
    }

    pub fn push(&mut self, item: T) {
        self.push_tree(Tree::from_child_trees(vec![Tree(Arc::new(
            Node::Leaf {
                summary: item.summarize(),
                items: SmallVec::from_vec(vec![item]),
            },
        ))]))
    }

    pub fn push_tree(&mut self, other: Self) {
        let other_node = other.0.clone();
        if !other_node.is_leaf() || other_node.items().len() > 0 {
            if self.0.height() < other_node.height() {
                for tree in other_node.child_trees() {
                    self.push_tree(tree.clone());
                }
            } else if let Some(split_tree) = self.push_tree_recursive(other) {
                *self = Self::from_child_trees(vec![self.clone(), split_tree]);
            }
        }
    }

    fn push_tree_recursive(&mut self, other: Tree<T>) -> Option<Tree<T>> {
        match Arc::make_mut(&mut self.0) {
            Node::Internal {
                height,
                summary,
                child_summaries,
                child_trees,
                ..
            } => {
                let other_node = other.0.clone();
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
                        child_trees.last().unwrap().0.summary().clone();

                    if let Some(split_tree) = tree_to_append {
                        summaries_to_append.push(split_tree.0.summary().clone());
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

                    Some(Tree(Arc::new(Node::Internal {
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
                let other_node = other.0;

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
                    Some(Tree(Arc::new(Node::Leaf {
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
        let height = child_trees[0].0.height() + 1;
        let mut child_summaries = SmallVec::new();
        for child in &child_trees {
            child_summaries.push(child.0.summary().clone());
        }
        let summary = sum(child_summaries.iter());
        Tree(Arc::new(Node::Internal {
            height,
            summary,
            child_summaries,
            child_trees: SmallVec::from_vec(child_trees),
        }))
    }

    fn leftmost_leaf(&self) -> Tree<T> {
        match *self.0 {
            Node::Leaf { .. } => self.clone(),
            Node::Internal {
                ref child_trees, ..
            } => child_trees.first().unwrap().leftmost_leaf(),
        }
    }

    fn rightmost_leaf(&self) -> Tree<T> {
        match *self.0 {
            Node::Leaf { .. } => self.clone(),
            Node::Internal {
                ref child_trees, ..
            } => child_trees.last().unwrap().rightmost_leaf(),
        }
    }
}

impl<T: KeyedItem> Tree<T> {
    pub fn insert(&mut self, item: T) {
        let mut cursor = self.cursor();
        let mut new_tree = cursor.slice(&item.key(), SeekBias::Left);
        new_tree.push(item);
        new_tree.push_tree(cursor.suffix::<T::Key>());
        *self = new_tree;
    }

    pub fn edit(&mut self, edits: &mut [Edit<T>]) {
        if edits.is_empty() {
            return;
        }

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
                    buffered_items.push(item.clone());
                }
                Edit::Remove(_) => {}
            }
        }

        new_tree.extend(buffered_items);
        new_tree.push_tree(cursor.suffix::<T::Key>());

        *self = new_tree;
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
                height: *height,
                summary: summary.clone(),
                child_summaries: child_summaries.clone(),
                child_trees: child_trees.clone(),
            },
            Node::Leaf { summary, items, .. } => Node::Leaf {
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
            at_end: false,
        }
    }

    fn reset(&mut self) {
        self.did_seek = false;
        self.at_end = false;
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
        if let Some((subtree, index, _)) = self.stack.last() {
            match *subtree.0 {
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
        if let Some((cur_leaf, index, _)) = self.stack.last() {
            if *index == 0 {
                if let Some(prev_leaf) = self.prev_leaf() {
                    let prev_leaf = prev_leaf.0;
                    Some(prev_leaf.items().last().unwrap().clone())
                } else {
                    None
                }
            } else {
                match *cur_leaf.0 {
                    Node::Leaf { ref items, .. } => Some(items[index - 1].clone()),
                    _ => unreachable!(),
                }
            }
        } else if self.at_end {
            self.tree.last()
        } else {
            None
        }
    }

    fn prev_leaf(&self) -> Option<Tree<T>> {
        for (ancestor, index, _) in self.stack.iter().rev().skip(1) {
            if *index != 0 {
                match *ancestor.0 {
                    Node::Internal {
                        ref child_trees, ..
                    } => return Some(child_trees[index - 1].rightmost_leaf()),
                    Node::Leaf { .. } => unreachable!(),
                };
            }
        }
        None
    }

    pub fn prev(&mut self) {
        assert!(self.did_seek, "Must seek before calling this method");

        if self.at_end {
            self.summary = T::Summary::default();
            let root = self.tree.clone();
            self.descend_to_last_item(root);
            self.at_end = false;
        } else {
            let search_result =
                self.stack
                    .iter()
                    .enumerate()
                    .rev()
                    .find_map(|(depth, (_, index, _))| {
                        if *index > 0 {
                            Some((depth, *index - 1))
                        } else {
                            None
                        }
                    });

            if let Some((depth, new_index)) = search_result {
                let subtree = self.stack.get(depth).unwrap().0.clone();
                self.stack.truncate(depth);
                self.summary = self
                    .stack
                    .last()
                    .map_or(T::Summary::default(), |(_, _, summary)| summary.clone());

                match subtree.0.as_ref() {
                    Node::Internal {
                        child_trees,
                        child_summaries,
                        ..
                    } => {
                        for summary in &child_summaries[0..new_index] {
                            self.summary += summary;
                        }
                        self.stack
                            .push((subtree.clone(), new_index, self.summary.clone()));
                        self.descend_to_last_item(child_trees[new_index].clone());
                    }
                    Node::Leaf { items, .. } => {
                        for item in &items[0..new_index] {
                            self.summary += &item.summarize();
                        }
                        self.stack
                            .push((subtree.clone(), new_index, self.summary.clone()));
                    }
                }
            }
        }
    }

    pub fn next(&mut self) {
        assert!(self.did_seek, "Must seek before calling this method");

        while self.stack.len() > 0 {
            let new_subtree = {
                let (subtree, index, summary) = self.stack.last_mut().unwrap();
                match subtree.0.as_ref() {
                    Node::Internal {
                        child_trees,
                        child_summaries,
                        ..
                    } => {
                        *summary += &child_summaries[*index];
                        *index += 1;
                        child_trees.get(*index).cloned()
                    }
                    Node::Leaf { items, .. } => {
                        let item_summary = items[*index].summarize();
                        self.summary += &item_summary;
                        *summary += &item_summary;
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
                self.descend_to_first_item(subtree);
                break;
            } else {
                self.stack.pop();
            }
        }

        self.at_end = self.stack.is_empty();
    }

    fn descend_to_first_item(&mut self, mut subtree: Tree<T>) {
        self.did_seek = true;
        loop {
            self.stack.push((subtree.clone(), 0, self.summary.clone()));
            subtree = match *subtree.0 {
                Node::Internal {
                    ref child_trees, ..
                } => child_trees[0].clone(),
                Node::Leaf { .. } => {
                    break;
                }
            }
        }
    }

    fn descend_to_last_item(&mut self, mut subtree: Tree<T>) {
        self.did_seek = true;
        loop {
            match subtree.0.clone().as_ref() {
                Node::Internal {
                    child_trees,
                    child_summaries,
                    ..
                } => {
                    for summary in &child_summaries[0..child_summaries.len() - 1] {
                        self.summary += summary;
                    }
                    self.stack
                        .push((subtree.clone(), child_trees.len() - 1, self.summary.clone()));
                    subtree = child_trees.last().unwrap().clone();
                }
                Node::Leaf { items, .. } => {
                    let last_index = items.len().saturating_sub(1);
                    for item in &items[0..last_index] {
                        self.summary += &item.summarize();
                    }
                    self.stack
                        .push((subtree.clone(), last_index, self.summary.clone()));
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
                    let (parent_subtree, index, _) = self.stack.last_mut().unwrap();
                    match *parent_subtree.0 {
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
                                        slice.push_tree(Tree(Arc::new(Node::Leaf {
                                            summary: slice_items_summary,
                                            items: slice_items,
                                        })));
                                    }
                                    break 'outer;
                                }
                            }

                            if let Some(slice) = slice.as_mut() {
                                if slice_items.len() > 0 {
                                    slice.push_tree(Tree(Arc::new(Node::Leaf {
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
                match *subtree.0 {
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
                                self.stack
                                    .push((subtree.clone(), index, self.summary.clone()));
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
                                self.stack
                                    .push((subtree.clone(), index, self.summary.clone()));
                                break;
                            }
                        }

                        if let Some(slice) = slice.as_mut() {
                            if slice_items.len() > 0 {
                                slice.push_tree(Tree(Arc::new(Node::Leaf {
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

        self.at_end = self.stack.is_empty();
        if bias == SeekBias::Left {
            *target == self.end::<D>()
        } else {
            *target == self.start::<D>()
        }
    }
}

impl<T: Item> Iterator for Cursor<T> {
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        if !self.did_seek {
            let root = self.tree.clone();
            self.descend_to_first_item(root);
        }

        if let Some(item) = self.item() {
            self.next();
            Some(item)
        } else {
            None
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
        for seed in 0..100 {
            use rand::{Rng, SeedableRng, StdRng};

            let mut rng = StdRng::from_seed(&[seed]);

            let mut tree = Tree::<u8>::new();
            let count = rng.gen_range(0, 10);
            tree.extend(rng.gen_iter().take(count));

            for _ in 0..5 {
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

                for i in 0..10 {
                    assert_eq!(cursor.start::<Count>().0, pos);

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

                    if i < 5 {
                        cursor.next();
                        if pos < reference_items.len() {
                            pos += 1;
                        }
                    } else {
                        cursor.prev();
                        pos = pos.saturating_sub(1);
                    }
                }
            }
        }
    }

    #[test]
    fn test_cursor() {
        // Empty tree
        let tree = Tree::<u8>::new();
        let mut cursor = tree.cursor();
        assert_eq!(cursor.slice(&Sum(0), SeekBias::Right).items(), vec![]);
        assert_eq!(cursor.item(), None);
        assert_eq!(cursor.prev_item(), None);
        assert_eq!(cursor.start::<Count>(), Count(0));
        assert_eq!(cursor.start::<Sum>(), Sum(0));

        // Single-element tree
        let mut tree = Tree::<u8>::new();
        tree.extend(vec![1]);
        let mut cursor = tree.cursor();
        assert_eq!(cursor.slice(&Sum(0), SeekBias::Right).items(), vec![]);
        assert_eq!(cursor.item(), Some(1));
        assert_eq!(cursor.prev_item(), None);
        assert_eq!(cursor.start::<Count>(), Count(0));
        assert_eq!(cursor.start::<Sum>(), Sum(0));

        cursor.next();
        assert_eq!(cursor.item(), None);
        assert_eq!(cursor.prev_item(), Some(1));
        assert_eq!(cursor.start::<Count>(), Count(1));
        assert_eq!(cursor.start::<Sum>(), Sum(1));

        cursor.prev();
        assert_eq!(cursor.item(), Some(1));
        assert_eq!(cursor.prev_item(), None);
        assert_eq!(cursor.start::<Count>(), Count(0));
        assert_eq!(cursor.start::<Sum>(), Sum(0));

        cursor.reset();
        assert_eq!(cursor.slice(&Sum(1), SeekBias::Right).items(), [1]);
        assert_eq!(cursor.item(), None);
        assert_eq!(cursor.prev_item(), Some(1));
        assert_eq!(cursor.start::<Count>(), Count(1));
        assert_eq!(cursor.start::<Sum>(), Sum(1));

        cursor.seek(&Sum(0), SeekBias::Right);
        assert_eq!(
            cursor
                .slice(&tree.extent::<Count>(), SeekBias::Right)
                .items(),
            [1]
        );
        assert_eq!(cursor.item(), None);
        assert_eq!(cursor.prev_item(), Some(1));
        assert_eq!(cursor.start::<Count>(), Count(1));
        assert_eq!(cursor.start::<Sum>(), Sum(1));

        // Multiple-element tree
        let mut tree = Tree::new();
        tree.extend(vec![1, 2, 3, 4, 5, 6]);
        let mut cursor = tree.cursor();

        assert_eq!(cursor.slice(&Sum(4), SeekBias::Right).items(), [1, 2]);
        assert_eq!(cursor.item(), Some(3));
        assert_eq!(cursor.prev_item(), Some(2));
        assert_eq!(cursor.start::<Count>(), Count(2));
        assert_eq!(cursor.start::<Sum>(), Sum(3));

        cursor.next();
        assert_eq!(cursor.item(), Some(4));
        assert_eq!(cursor.prev_item(), Some(3));
        assert_eq!(cursor.start::<Count>(), Count(3));
        assert_eq!(cursor.start::<Sum>(), Sum(6));

        cursor.next();
        assert_eq!(cursor.item(), Some(5));
        assert_eq!(cursor.prev_item(), Some(4));
        assert_eq!(cursor.start::<Count>(), Count(4));
        assert_eq!(cursor.start::<Sum>(), Sum(10));

        cursor.next();
        assert_eq!(cursor.item(), Some(6));
        assert_eq!(cursor.prev_item(), Some(5));
        assert_eq!(cursor.start::<Count>(), Count(5));
        assert_eq!(cursor.start::<Sum>(), Sum(15));

        cursor.next();
        cursor.next();
        assert_eq!(cursor.item(), None);
        assert_eq!(cursor.prev_item(), Some(6));
        assert_eq!(cursor.start::<Count>(), Count(6));
        assert_eq!(cursor.start::<Sum>(), Sum(21));

        cursor.prev();
        assert_eq!(cursor.item(), Some(6));
        assert_eq!(cursor.prev_item(), Some(5));
        assert_eq!(cursor.start::<Count>(), Count(5));
        assert_eq!(cursor.start::<Sum>(), Sum(15));

        cursor.prev();
        assert_eq!(cursor.item(), Some(5));
        assert_eq!(cursor.prev_item(), Some(4));
        assert_eq!(cursor.start::<Count>(), Count(4));
        assert_eq!(cursor.start::<Sum>(), Sum(10));

        cursor.prev();
        assert_eq!(cursor.item(), Some(4));
        assert_eq!(cursor.prev_item(), Some(3));
        assert_eq!(cursor.start::<Count>(), Count(3));
        assert_eq!(cursor.start::<Sum>(), Sum(6));

        cursor.prev();
        assert_eq!(cursor.item(), Some(3));
        assert_eq!(cursor.prev_item(), Some(2));
        assert_eq!(cursor.start::<Count>(), Count(2));
        assert_eq!(cursor.start::<Sum>(), Sum(3));

        cursor.prev();
        assert_eq!(cursor.item(), Some(2));
        assert_eq!(cursor.prev_item(), Some(1));
        assert_eq!(cursor.start::<Count>(), Count(1));
        assert_eq!(cursor.start::<Sum>(), Sum(1));

        cursor.prev();
        assert_eq!(cursor.item(), Some(1));
        assert_eq!(cursor.prev_item(), None);
        assert_eq!(cursor.start::<Count>(), Count(0));
        assert_eq!(cursor.start::<Sum>(), Sum(0));

        cursor.prev();
        assert_eq!(cursor.item(), Some(1));
        assert_eq!(cursor.prev_item(), None);
        assert_eq!(cursor.start::<Count>(), Count(0));
        assert_eq!(cursor.start::<Sum>(), Sum(0));

        cursor.reset();
        assert_eq!(
            cursor
                .slice(&tree.extent::<Count>(), SeekBias::Right)
                .items(),
            tree.items()
        );
        assert_eq!(cursor.item(), None);
        assert_eq!(cursor.prev_item(), Some(6));
        assert_eq!(cursor.start::<Count>(), Count(6));
        assert_eq!(cursor.start::<Sum>(), Sum(21));

        cursor.seek(&Count(3), SeekBias::Right);
        assert_eq!(
            cursor
                .slice(&tree.extent::<Count>(), SeekBias::Right)
                .items(),
            [4, 5, 6]
        );
        assert_eq!(cursor.item(), None);
        assert_eq!(cursor.prev_item(), Some(6));
        assert_eq!(cursor.start::<Count>(), Count(6));
        assert_eq!(cursor.start::<Sum>(), Sum(21));

        // Seeking can bias left or right
        cursor.seek(&Sum(1), SeekBias::Left);
        assert_eq!(cursor.item(), Some(1));
        cursor.seek(&Sum(1), SeekBias::Right);
        assert_eq!(cursor.item(), Some(2));

        // Slicing without resetting starts from where the cursor is parked at.
        cursor.seek(&Sum(1), SeekBias::Right);
        assert_eq!(cursor.slice(&Sum(6), SeekBias::Right).items(), vec![2, 3]);
        assert_eq!(cursor.slice(&Sum(21), SeekBias::Left).items(), vec![4, 5]);
        assert_eq!(cursor.slice(&Sum(21), SeekBias::Right).items(), vec![6]);
    }

    #[derive(Clone, Default, Debug)]
    pub struct IntegersSummary {
        count: Count,
        sum: Sum,
    }

    #[derive(Ord, PartialOrd, Default, Eq, PartialEq, Clone, Debug)]
    struct Count(usize);

    #[derive(Ord, PartialOrd, Default, Eq, PartialEq, Clone, Debug)]
    struct Sum(usize);

    impl Item for u8 {
        type Summary = IntegersSummary;

        fn summarize(&self) -> Self::Summary {
            IntegersSummary {
                count: Count(1),
                sum: Sum(*self as usize),
            }
        }
    }

    impl<'a> AddAssign<&'a Self> for IntegersSummary {
        fn add_assign(&mut self, other: &Self) {
            self.count += &other.count;
            self.sum += &other.sum;
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

    impl Dimension<IntegersSummary> for Sum {
        fn from_summary(summary: &IntegersSummary) -> Self {
            summary.sum.clone()
        }
    }

    impl<'a> AddAssign<&'a Self> for Sum {
        fn add_assign(&mut self, other: &Self) {
            self.0 += other.0;
        }
    }

    impl<'a> Add<&'a Self> for Sum {
        type Output = Self;

        fn add(mut self, other: &Self) -> Self {
            self.0 += other.0;
            self
        }
    }
}
