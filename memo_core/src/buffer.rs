use crate::btree::{self, SeekBias};
use crate::operation_queue::{self, OperationQueue};
use crate::serialization;
use crate::time;
use crate::{Error, ReplicaId};
use difference::{Changeset, Difference};
use flatbuffers::{FlatBufferBuilder, WIPOffset};
use lazy_static::lazy_static;
use serde_derive::{Deserialize, Serialize};
use smallvec::SmallVec;
use std::cell::RefCell;
use std::cmp::{self, Ordering};
use std::collections::{HashMap, HashSet};
use std::iter;
use std::mem;
use std::ops::{Add, AddAssign, Range, Sub};
use std::sync::Arc;
use std::vec;

pub type SelectionSetId = time::Local;

#[derive(Clone)]
pub struct Buffer {
    fragments: btree::Tree<Fragment>,
    insertion_splits: HashMap<time::Local, btree::Tree<InsertionSplit>>,
    anchor_cache: RefCell<HashMap<Anchor, (usize, Point)>>,
    offset_cache: RefCell<HashMap<Point, usize>>,
    pub version: time::Global,
    selections: HashMap<SelectionSetId, Vec<Selection>>,
    selections_last_update: time::Local,
    deferred_ops: OperationQueue<Operation>,
    deferred_replicas: HashSet<ReplicaId>,
}

#[derive(Clone, Copy, Deserialize, Eq, PartialEq, Debug, Hash, Serialize)]
pub struct Point {
    pub row: u32,
    pub column: u32,
}

#[derive(Clone, Eq, PartialEq, Debug, Hash)]
pub enum Anchor {
    Start,
    End,
    Middle {
        insertion_id: time::Local,
        offset: usize,
        bias: AnchorBias,
    },
}

#[derive(Clone, Eq, PartialEq, Debug, Hash)]
pub enum AnchorBias {
    Left,
    Right,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Selection {
    pub start: Anchor,
    pub end: Anchor,
    pub reversed: bool,
}

pub struct Iter {
    fragment_cursor: btree::Cursor<Fragment>,
    fragment_offset: usize,
    reversed: bool,
}

struct ChangesIter<F: Fn(&FragmentSummary) -> bool> {
    cursor: btree::FilterCursor<F, Fragment>,
    since: time::Global,
}

struct DiffIter {
    position: Point,
    diff: vec::IntoIter<Difference>,
}

#[derive(Debug, Eq, PartialEq)]
pub struct Change {
    pub range: Range<Point>,
    pub code_units: Vec<u16>,
    new_extent: Point,
}

#[derive(Clone, Eq, PartialEq, Debug)]
pub struct Insertion {
    id: time::Local,
    parent_id: time::Local,
    offset_in_parent: usize,
    text: Arc<Text>,
    lamport_timestamp: time::Lamport,
}

#[derive(Clone, Eq, PartialEq, Debug)]
pub struct Text {
    code_units: Vec<u16>,
    nodes: Vec<LineNode>,
}

#[derive(Clone, Eq, PartialEq, Debug)]
struct LineNode {
    len: u32,
    longest_row: u32,
    longest_row_len: u32,
    offset: usize,
    rows: u32,
}

struct LineNodeProbe<'a> {
    offset_range: &'a Range<usize>,
    row: u32,
    left_ancestor_end_offset: usize,
    right_ancestor_start_offset: usize,
    node: &'a LineNode,
    left_child: Option<&'a LineNode>,
    right_child: Option<&'a LineNode>,
}

#[derive(Ord, PartialOrd, Eq, PartialEq, Clone, Debug)]
struct FragmentId(Arc<Vec<u16>>);

#[derive(Eq, PartialEq, Clone, Debug)]
struct Fragment {
    id: FragmentId,
    insertion: Insertion,
    start_offset: usize,
    end_offset: usize,
    deletions: HashSet<time::Local>,
}

#[derive(Eq, PartialEq, Clone, Debug)]
pub struct FragmentSummary {
    extent: usize,
    extent_2d: Point,
    max_fragment_id: FragmentId,
    first_row_len: u32,
    longest_row: u32,
    longest_row_len: u32,
    max_version: time::Global,
}

#[derive(Eq, PartialEq, Clone, Debug)]
struct InsertionSplit {
    extent: usize,
    fragment_id: FragmentId,
}

#[derive(Eq, PartialEq, Clone, Debug)]
struct InsertionSplitSummary {
    extent: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Operation {
    Edit {
        start_id: time::Local,
        start_offset: usize,
        end_id: time::Local,
        end_offset: usize,
        version_in_range: time::Global,
        new_text: Option<Arc<Text>>,
        local_timestamp: time::Local,
        lamport_timestamp: time::Lamport,
    },
    UpdateSelections {
        set_id: time::Local,
        selections: Option<Vec<Selection>>,
        local_timestamp: time::Local,
        lamport_timestamp: time::Lamport,
    },
}

impl Buffer {
    pub fn new<T>(base_text: T) -> Self
    where
        T: Into<Text>,
    {
        let mut insertion_splits = HashMap::new();
        let mut fragments = btree::Tree::new();

        let base_insertion = Insertion {
            id: time::Local::default(),
            parent_id: time::Local::default(),
            offset_in_parent: 0,
            text: Arc::new(base_text.into()),
            lamport_timestamp: time::Lamport::default(),
        };

        insertion_splits.insert(
            base_insertion.id,
            btree::Tree::from_item(InsertionSplit {
                fragment_id: FragmentId::min_value(),
                extent: 0,
            }),
        );
        fragments.push(Fragment {
            id: FragmentId::min_value(),
            insertion: base_insertion.clone(),
            start_offset: 0,
            end_offset: 0,
            deletions: HashSet::new(),
        });

        if base_insertion.text.len() > 0 {
            let base_fragment_id =
                FragmentId::between(&FragmentId::min_value(), &FragmentId::max_value());

            insertion_splits
                .get_mut(&base_insertion.id)
                .unwrap()
                .push(InsertionSplit {
                    fragment_id: base_fragment_id.clone(),
                    extent: base_insertion.text.len(),
                });
            fragments.push(Fragment {
                id: base_fragment_id,
                start_offset: 0,
                end_offset: base_insertion.text.len(),
                insertion: base_insertion,
                deletions: HashSet::new(),
            });
        }

        Self {
            fragments,
            insertion_splits,
            anchor_cache: RefCell::new(HashMap::default()),
            offset_cache: RefCell::new(HashMap::default()),
            version: time::Global::new(),
            selections: HashMap::default(),
            selections_last_update: time::Local::default(),
            deferred_ops: OperationQueue::new(),
            deferred_replicas: HashSet::new(),
        }
    }

    pub fn is_modified(&self) -> bool {
        self.version > time::Global::new()
    }

    pub fn len(&self) -> usize {
        self.fragments.extent::<usize>()
    }

    pub fn len_for_row(&self, row: u32) -> Result<u32, Error> {
        let row_start_offset = self.offset_for_point(Point::new(row, 0))?;
        let row_end_offset = if row >= self.max_point().row {
            self.len()
        } else {
            self.offset_for_point(Point::new(row + 1, 0))? - 1
        };

        Ok((row_end_offset - row_start_offset) as u32)
    }

    pub fn longest_row(&self) -> u32 {
        self.fragments.summary().longest_row
    }

    pub fn max_point(&self) -> Point {
        self.fragments.extent()
    }

    pub fn line(&self, row: u32) -> Result<Vec<u16>, Error> {
        let mut iterator = self.iter_at_point(Point::new(row, 0)).peekable();
        if iterator.peek().is_none() {
            Err(Error::OffsetOutOfRange)
        } else {
            Ok(iterator.take_while(|c| *c != u16::from(b'\n')).collect())
        }
    }

    pub fn to_u16_chars(&self) -> Vec<u16> {
        self.iter().collect::<Vec<u16>>()
    }

    pub fn to_string(&self) -> String {
        String::from_utf16_lossy(&self.to_u16_chars())
    }

    pub fn iter(&self) -> Iter {
        Iter::new(self)
    }

    pub fn iter_at_point(&self, point: Point) -> Iter {
        Iter::at_point(self, point)
    }

    pub fn selections_changed_since(&self, since: &time::Global) -> bool {
        since.observed(self.selections_last_update)
    }

    pub fn changes_since(&self, since: &time::Global) -> impl Iterator<Item = Change> {
        let since_2 = since.clone();
        let cursor = self
            .fragments
            .filter(move |summary| summary.max_version.changed_since(&since_2));
        ChangesIter {
            cursor,
            since: since.clone(),
        }
    }

    pub fn edit<I, T>(
        &mut self,
        old_ranges: I,
        new_text: T,
        local_clock: &mut time::Local,
        lamport_clock: &mut time::Lamport,
    ) -> Vec<Operation>
    where
        I: IntoIterator<Item = Range<usize>>,
        T: Into<Text>,
    {
        let new_text = new_text.into();
        let new_text = if new_text.len() > 0 {
            Some(Arc::new(new_text))
        } else {
            None
        };

        self.anchor_cache.borrow_mut().clear();
        self.offset_cache.borrow_mut().clear();
        let ops = self.splice_fragments(
            old_ranges
                .into_iter()
                .filter(|old_range| new_text.is_some() || old_range.end > old_range.start),
            new_text.clone(),
            local_clock,
            lamport_clock,
        );
        if let Some(op) = ops.last() {
            match op {
                Operation::Edit {
                    local_timestamp, ..
                } => self.version.observe(*local_timestamp),
                _ => unreachable!(),
            }
        }
        ops
    }

    pub fn edit_2d<I, T>(
        &mut self,
        old_2d_ranges: I,
        new_text: T,
        local_clock: &mut time::Local,
        lamport_clock: &mut time::Lamport,
    ) -> Vec<Operation>
    where
        I: IntoIterator<Item = Range<Point>>,
        T: Into<Text>,
    {
        let mut old_1d_ranges = SmallVec::<[_; 1]>::new();
        for old_2d_range in old_2d_ranges {
            let start = self.offset_for_point(old_2d_range.start);
            let end = self.offset_for_point(old_2d_range.end);
            if start.is_ok() && end.is_ok() {
                old_1d_ranges.push(start.unwrap()..end.unwrap());
            }
        }
        self.edit(old_1d_ranges, new_text, local_clock, lamport_clock)
    }

    pub fn add_selection_set(
        &mut self,
        selections: Vec<Selection>,
        local_clock: &mut time::Local,
        lamport_clock: &mut time::Lamport,
    ) -> (SelectionSetId, Operation) {
        let set_id = local_clock.tick();
        self.selections.insert(set_id, selections.clone());
        (
            set_id,
            Operation::UpdateSelections {
                set_id,
                selections: Some(selections),
                local_timestamp: local_clock.tick(),
                lamport_timestamp: lamport_clock.tick(),
            },
        )
    }

    pub fn remove_selection_set(
        &mut self,
        set_id: SelectionSetId,
        local_clock: &mut time::Local,
        lamport_clock: &mut time::Lamport,
    ) -> Result<Operation, Error> {
        self.selections
            .remove(&set_id)
            .ok_or(Error::InvalidSelectionSet)?;
        Ok(Operation::UpdateSelections {
            set_id,
            selections: None,
            local_timestamp: local_clock.tick(),
            lamport_timestamp: lamport_clock.tick(),
        })
    }

    pub fn selections(&self) -> impl Iterator<Item = (&SelectionSetId, &Vec<Selection>)> {
        self.selections.iter()
    }

    pub fn mutate_selections<F>(
        &mut self,
        set_id: SelectionSetId,
        local_clock: &mut time::Local,
        lamport_clock: &mut time::Lamport,
        f: F,
    ) -> Result<Operation, Error>
    where
        F: FnOnce(&Buffer, &mut Vec<Selection>),
    {
        let mut selections = self
            .selections
            .remove(&set_id)
            .ok_or(Error::InvalidSelectionSet)?;
        f(self, &mut selections);
        self.merge_selections(&mut selections);
        self.selections.insert(set_id, selections.clone());
        Ok(Operation::UpdateSelections {
            set_id,
            selections: Some(selections),
            local_timestamp: local_clock.tick(),
            lamport_timestamp: lamport_clock.tick(),
        })
    }

    fn merge_selections(&mut self, selections: &mut Vec<Selection>) {
        let mut new_selections = Vec::with_capacity(selections.len());
        {
            let mut old_selections = selections.drain(..);
            if let Some(mut prev_selection) = old_selections.next() {
                for selection in old_selections {
                    if self
                        .cmp_anchors(&prev_selection.end, &selection.start)
                        .unwrap()
                        >= Ordering::Equal
                    {
                        if self
                            .cmp_anchors(&selection.end, &prev_selection.end)
                            .unwrap()
                            > Ordering::Equal
                        {
                            prev_selection.end = selection.end;
                        }
                    } else {
                        new_selections.push(mem::replace(&mut prev_selection, selection));
                    }
                }
                new_selections.push(prev_selection);
            }
        }
        *selections = new_selections;
    }

    pub fn apply_ops<I: IntoIterator<Item = Operation>>(
        &mut self,
        ops: I,
        local_clock: &mut time::Local,
        lamport_clock: &mut time::Lamport,
    ) -> Result<(), Error> {
        let mut deferred_ops = Vec::new();
        for op in ops {
            if self.can_apply_op(&op) {
                self.apply_op(op, local_clock, lamport_clock)?;
            } else {
                self.deferred_replicas.insert(op.replica_id());
                deferred_ops.push(op);
            }
        }
        self.deferred_ops.insert(deferred_ops);
        self.flush_deferred_ops(local_clock, lamport_clock)?;
        Ok(())
    }

    fn apply_op(
        &mut self,
        op: Operation,
        local_clock: &mut time::Local,
        lamport_clock: &mut time::Lamport,
    ) -> Result<(), Error> {
        match op {
            Operation::Edit {
                start_id,
                start_offset,
                end_id,
                end_offset,
                new_text,
                version_in_range,
                local_timestamp,
                lamport_timestamp,
            } => {
                self.apply_edit(
                    start_id,
                    start_offset,
                    end_id,
                    end_offset,
                    new_text.as_ref().cloned(),
                    &version_in_range,
                    local_timestamp,
                    lamport_timestamp,
                    local_clock,
                    lamport_clock,
                )?;
                self.anchor_cache.borrow_mut().clear();
                self.offset_cache.borrow_mut().clear();
            }
            Operation::UpdateSelections {
                set_id,
                selections,
                local_timestamp,
                lamport_timestamp,
            } => {
                if let Some(selections) = selections {
                    self.selections.insert(set_id, selections);
                } else {
                    self.selections.remove(&set_id);
                }
                local_clock.observe(set_id);
                lamport_clock.observe(lamport_timestamp);
                self.selections_last_update = local_timestamp;
            }
        }
        Ok(())
    }

    fn apply_edit(
        &mut self,
        start_id: time::Local,
        start_offset: usize,
        end_id: time::Local,
        end_offset: usize,
        new_text: Option<Arc<Text>>,
        version_in_range: &time::Global,
        local_timestamp: time::Local,
        lamport_timestamp: time::Lamport,
        local_clock: &mut time::Local,
        lamport_clock: &mut time::Lamport,
    ) -> Result<(), Error> {
        if self.version.observed(local_timestamp) {
            return Ok(());
        }

        let mut new_text = new_text.as_ref().cloned();
        let start_fragment_id = self.resolve_fragment_id(start_id, start_offset)?;
        let end_fragment_id = self.resolve_fragment_id(end_id, end_offset)?;

        let old_fragments = self.fragments.clone();
        let mut cursor = old_fragments.cursor();
        let mut new_fragments = cursor.slice(&start_fragment_id, SeekBias::Left);

        if start_offset == cursor.item().unwrap().end_offset {
            new_fragments.push(cursor.item().unwrap());
            cursor.next();
        }

        while let Some(mut fragment) = cursor.item() {
            if new_text.is_none() && fragment.id > end_fragment_id {
                break;
            }

            if fragment.id == start_fragment_id || fragment.id == end_fragment_id {
                let split_start = if start_fragment_id == fragment.id {
                    start_offset
                } else {
                    fragment.start_offset
                };
                let split_end = if end_fragment_id == fragment.id {
                    end_offset
                } else {
                    fragment.end_offset
                };
                let (before_range, within_range, after_range) = self.split_fragment(
                    cursor.prev_item().as_ref().unwrap(),
                    &fragment,
                    split_start..split_end,
                );
                let insertion = if let Some(new_text) = new_text.take() {
                    Some(
                        self.build_fragment_to_insert(
                            before_range
                                .as_ref()
                                .or(cursor.prev_item().as_ref())
                                .unwrap(),
                            within_range.as_ref().or(after_range.as_ref()),
                            new_text,
                            local_timestamp,
                            lamport_timestamp,
                        ),
                    )
                } else {
                    None
                };
                if let Some(fragment) = before_range {
                    new_fragments.push(fragment);
                }
                if let Some(fragment) = insertion {
                    new_fragments.push(fragment);
                }
                if let Some(mut fragment) = within_range {
                    if version_in_range.observed(fragment.insertion.id) {
                        fragment.deletions.insert(local_timestamp);
                    }
                    new_fragments.push(fragment);
                }
                if let Some(fragment) = after_range {
                    new_fragments.push(fragment);
                }
            } else {
                if new_text.is_some() && lamport_timestamp > fragment.insertion.lamport_timestamp {
                    new_fragments.push(self.build_fragment_to_insert(
                        cursor.prev_item().as_ref().unwrap(),
                        Some(&fragment),
                        new_text.take().unwrap(),
                        local_timestamp,
                        lamport_timestamp,
                    ));
                }

                if fragment.id < end_fragment_id && version_in_range.observed(fragment.insertion.id)
                {
                    fragment.deletions.insert(local_timestamp);
                }
                new_fragments.push(fragment);
            }

            cursor.next();
        }

        if let Some(new_text) = new_text {
            new_fragments.push(self.build_fragment_to_insert(
                cursor.prev_item().as_ref().unwrap(),
                None,
                new_text,
                local_timestamp,
                lamport_timestamp,
            ));
        }

        new_fragments.push_tree(cursor.slice(&old_fragments.extent::<usize>(), SeekBias::Right));
        self.fragments = new_fragments;
        self.version.observe(local_timestamp);
        local_clock.observe(local_timestamp);
        lamport_clock.observe(lamport_timestamp);
        Ok(())
    }

    fn flush_deferred_ops(
        &mut self,
        local_clock: &mut time::Local,
        lamport_clock: &mut time::Lamport,
    ) -> Result<(), Error> {
        self.deferred_replicas.clear();
        let mut deferred_ops = Vec::new();
        for op in self.deferred_ops.drain() {
            if self.can_apply_op(&op) {
                self.apply_op(op, local_clock, lamport_clock)?;
            } else {
                self.deferred_replicas.insert(op.replica_id());
                deferred_ops.push(op);
            }
        }
        self.deferred_ops.insert(deferred_ops);
        Ok(())
    }

    fn can_apply_op(&self, op: &Operation) -> bool {
        if self.deferred_replicas.contains(&op.replica_id()) {
            false
        } else {
            match op {
                Operation::Edit {
                    start_id,
                    end_id,
                    version_in_range,
                    ..
                } => {
                    self.version.observed(*start_id)
                        && self.version.observed(*end_id)
                        && *version_in_range <= self.version
                }
                Operation::UpdateSelections { selections, .. } => {
                    if let Some(selections) = selections {
                        selections.iter().all(|selection| {
                            let contains_start = match selection.start {
                                Anchor::Middle { insertion_id, .. } => {
                                    self.version.observed(insertion_id)
                                }
                                _ => true,
                            };
                            let contains_end = match selection.end {
                                Anchor::Middle { insertion_id, .. } => {
                                    self.version.observed(insertion_id)
                                }
                                _ => true,
                            };
                            contains_start && contains_end
                        })
                    } else {
                        true
                    }
                }
            }
        }
    }

    fn resolve_fragment_id(
        &self,
        edit_id: time::Local,
        offset: usize,
    ) -> Result<FragmentId, Error> {
        let split_tree = self
            .insertion_splits
            .get(&edit_id)
            .ok_or(Error::InvalidOperation)?;
        let mut cursor = split_tree.cursor();
        cursor.seek(&offset, SeekBias::Left);
        Ok(cursor
            .item()
            .ok_or(Error::InvalidOperation)?
            .fragment_id
            .clone())
    }

    fn splice_fragments<I>(
        &mut self,
        mut old_ranges: I,
        new_text: Option<Arc<Text>>,
        local_clock: &mut time::Local,
        lamport_clock: &mut time::Lamport,
    ) -> Vec<Operation>
    where
        I: Iterator<Item = Range<usize>>,
    {
        let mut cur_range = old_ranges.next();
        if cur_range.is_none() {
            return Vec::new();
        }

        let mut ops = Vec::with_capacity(old_ranges.size_hint().0);

        let old_fragments = self.fragments.clone();
        let mut cursor = old_fragments.cursor();
        let mut new_fragments = btree::Tree::new();
        new_fragments.push_tree(cursor.slice(&cur_range.as_ref().unwrap().start, SeekBias::Right));

        let mut start_id = None;
        let mut start_offset = None;
        let mut end_id = None;
        let mut end_offset = None;
        let mut version_in_range = time::Global::new();

        let mut local_timestamp = local_clock.tick();
        let mut lamport_timestamp = lamport_clock.tick();

        while cur_range.is_some() && cursor.item().is_some() {
            let mut fragment = cursor.item().unwrap();
            let mut fragment_start = cursor.start::<usize>();
            let mut fragment_end = fragment_start + fragment.len();

            let old_split_tree = self
                .insertion_splits
                .remove(&fragment.insertion.id)
                .unwrap();
            let mut splits_cursor = old_split_tree.cursor();
            let mut new_split_tree = splits_cursor.slice(&fragment.start_offset, SeekBias::Right);

            // Find all splices that start or end within the current fragment. Then, split the
            // fragment and reassemble it in both trees accounting for the deleted and the newly
            // inserted text.
            while cur_range.as_ref().map_or(false, |r| r.start < fragment_end) {
                let range = cur_range.clone().unwrap();
                if range.start > fragment_start {
                    let mut prefix = fragment.clone();
                    prefix.end_offset = prefix.start_offset + (range.start - fragment_start);
                    prefix.id =
                        FragmentId::between(&new_fragments.last().unwrap().id, &fragment.id);
                    fragment.start_offset = prefix.end_offset;
                    new_fragments.push(prefix.clone());
                    new_split_tree.push(InsertionSplit {
                        extent: prefix.end_offset - prefix.start_offset,
                        fragment_id: prefix.id,
                    });
                    fragment_start = range.start;
                }

                if range.end == fragment_start {
                    end_id = Some(new_fragments.last().unwrap().insertion.id);
                    end_offset = Some(new_fragments.last().unwrap().end_offset);
                } else if range.end == fragment_end {
                    end_id = Some(fragment.insertion.id);
                    end_offset = Some(fragment.end_offset);
                }

                if range.start == fragment_start {
                    start_id = Some(new_fragments.last().unwrap().insertion.id);
                    start_offset = Some(new_fragments.last().unwrap().end_offset);

                    if let Some(new_text) = new_text.clone() {
                        let new_fragment = self.build_fragment_to_insert(
                            &new_fragments.last().unwrap(),
                            Some(&fragment),
                            new_text,
                            local_timestamp,
                            lamport_timestamp,
                        );
                        new_fragments.push(new_fragment);
                    }
                }

                if range.end < fragment_end {
                    if range.end > fragment_start {
                        let mut prefix = fragment.clone();
                        prefix.end_offset = prefix.start_offset + (range.end - fragment_start);
                        prefix.id =
                            FragmentId::between(&new_fragments.last().unwrap().id, &fragment.id);
                        if fragment.is_visible() {
                            prefix.deletions.insert(local_timestamp);
                        }
                        fragment.start_offset = prefix.end_offset;
                        new_fragments.push(prefix.clone());
                        new_split_tree.push(InsertionSplit {
                            extent: prefix.end_offset - prefix.start_offset,
                            fragment_id: prefix.id,
                        });
                        fragment_start = range.end;
                        end_id = Some(fragment.insertion.id);
                        end_offset = Some(fragment.start_offset);
                        version_in_range.observe(fragment.insertion.id);
                    }
                } else {
                    version_in_range.observe(fragment.insertion.id);
                    if fragment.is_visible() {
                        fragment.deletions.insert(local_timestamp);
                    }
                }

                // If the splice ends inside this fragment, we can advance to the next splice and
                // check if it also intersects the current fragment. Otherwise we break out of the
                // loop and find the first fragment that the splice does not contain fully.
                if range.end <= fragment_end {
                    ops.push(Operation::Edit {
                        start_id: start_id.unwrap(),
                        start_offset: start_offset.unwrap(),
                        end_id: end_id.unwrap(),
                        end_offset: end_offset.unwrap(),
                        version_in_range,
                        new_text: new_text.clone(),
                        local_timestamp,
                        lamport_timestamp,
                    });

                    start_id = None;
                    start_offset = None;
                    end_id = None;
                    end_offset = None;
                    version_in_range = time::Global::new();
                    cur_range = old_ranges.next();
                    if cur_range.is_some() {
                        local_timestamp = local_clock.tick();
                        lamport_timestamp = lamport_clock.tick();
                    }
                } else {
                    break;
                }
            }
            new_split_tree.push(InsertionSplit {
                extent: fragment.end_offset - fragment.start_offset,
                fragment_id: fragment.id.clone(),
            });
            splits_cursor.next();
            new_split_tree
                .push_tree(splits_cursor.slice(&old_split_tree.extent::<usize>(), SeekBias::Right));
            self.insertion_splits
                .insert(fragment.insertion.id, new_split_tree);
            new_fragments.push(fragment);

            // Scan forward until we find a fragment that is not fully contained by the current splice.
            cursor.next();
            if let Some(range) = cur_range.clone() {
                while let Some(mut fragment) = cursor.item() {
                    fragment_start = cursor.start::<usize>();
                    fragment_end = fragment_start + fragment.len();
                    if range.start < fragment_start && range.end >= fragment_end {
                        if fragment.is_visible() {
                            fragment.deletions.insert(local_timestamp);
                        }
                        version_in_range.observe(fragment.insertion.id);
                        new_fragments.push(fragment.clone());
                        cursor.next();

                        if range.end == fragment_end {
                            end_id = Some(fragment.insertion.id);
                            end_offset = Some(fragment.end_offset);
                            ops.push(Operation::Edit {
                                start_id: start_id.unwrap(),
                                start_offset: start_offset.unwrap(),
                                end_id: end_id.unwrap(),
                                end_offset: end_offset.unwrap(),
                                version_in_range,
                                new_text: new_text.clone(),
                                local_timestamp,
                                lamport_timestamp,
                            });

                            start_id = None;
                            start_offset = None;
                            end_id = None;
                            end_offset = None;
                            version_in_range = time::Global::new();

                            cur_range = old_ranges.next();
                            if cur_range.is_some() {
                                local_timestamp = local_clock.tick();
                                lamport_timestamp = lamport_clock.tick();
                            }
                            break;
                        }
                    } else {
                        break;
                    }
                }

                // If the splice we are currently evaluating starts after the end of the fragment
                // that the cursor is parked at, we should seek to the next splice's start range
                // and push all the fragments in between into the new tree.
                if cur_range.as_ref().map_or(false, |r| r.start > fragment_end) {
                    new_fragments.push_tree(
                        cursor.slice(&cur_range.as_ref().unwrap().start, SeekBias::Right),
                    );
                }
            }
        }

        // Handle range that is at the end of the buffer if it exists. There should never be
        // multiple because ranges must be disjoint.
        if cur_range.is_some() {
            debug_assert_eq!(old_ranges.next(), None);
            let last_fragment = new_fragments.last().unwrap();
            ops.push(Operation::Edit {
                start_id: last_fragment.insertion.id,
                start_offset: last_fragment.end_offset,
                end_id: last_fragment.insertion.id,
                end_offset: last_fragment.end_offset,
                version_in_range: time::Global::new(),
                new_text: new_text.clone(),
                local_timestamp,
                lamport_timestamp,
            });

            if let Some(new_text) = new_text {
                new_fragments.push(self.build_fragment_to_insert(
                    &last_fragment,
                    None,
                    new_text,
                    local_timestamp,
                    lamport_timestamp,
                ));
            }
        } else {
            new_fragments
                .push_tree(cursor.slice(&old_fragments.extent::<usize>(), SeekBias::Right));
        }

        self.fragments = new_fragments;
        ops
    }

    fn split_fragment(
        &mut self,
        prev_fragment: &Fragment,
        fragment: &Fragment,
        range: Range<usize>,
    ) -> (Option<Fragment>, Option<Fragment>, Option<Fragment>) {
        debug_assert!(range.start >= fragment.start_offset);
        debug_assert!(range.start <= fragment.end_offset);
        debug_assert!(range.end <= fragment.end_offset);
        debug_assert!(range.end >= fragment.start_offset);

        if range.end == fragment.start_offset {
            (None, None, Some(fragment.clone()))
        } else if range.start == fragment.end_offset {
            (Some(fragment.clone()), None, None)
        } else if range.start == fragment.start_offset && range.end == fragment.end_offset {
            (None, Some(fragment.clone()), None)
        } else {
            let mut prefix = fragment.clone();

            let after_range = if range.end < fragment.end_offset {
                let mut suffix = prefix.clone();
                suffix.start_offset = range.end;
                prefix.end_offset = range.end;
                prefix.id = FragmentId::between(&prev_fragment.id, &suffix.id);
                Some(suffix)
            } else {
                None
            };

            let within_range = if range.start != range.end {
                let mut suffix = prefix.clone();
                suffix.start_offset = range.start;
                prefix.end_offset = range.start;
                prefix.id = FragmentId::between(&prev_fragment.id, &suffix.id);
                Some(suffix)
            } else {
                None
            };

            let before_range = if range.start > fragment.start_offset {
                Some(prefix)
            } else {
                None
            };

            let old_split_tree = self
                .insertion_splits
                .remove(&fragment.insertion.id)
                .unwrap();
            let mut cursor = old_split_tree.cursor();
            let mut new_split_tree = cursor.slice(&fragment.start_offset, SeekBias::Right);

            if let Some(ref fragment) = before_range {
                new_split_tree.push(InsertionSplit {
                    extent: range.start - fragment.start_offset,
                    fragment_id: fragment.id.clone(),
                });
            }

            if let Some(ref fragment) = within_range {
                new_split_tree.push(InsertionSplit {
                    extent: range.end - range.start,
                    fragment_id: fragment.id.clone(),
                });
            }

            if let Some(ref fragment) = after_range {
                new_split_tree.push(InsertionSplit {
                    extent: fragment.end_offset - range.end,
                    fragment_id: fragment.id.clone(),
                });
            }

            cursor.next();
            new_split_tree
                .push_tree(cursor.slice(&old_split_tree.extent::<usize>(), SeekBias::Right));

            self.insertion_splits
                .insert(fragment.insertion.id, new_split_tree);

            (before_range, within_range, after_range)
        }
    }

    fn build_fragment_to_insert(
        &mut self,
        prev_fragment: &Fragment,
        next_fragment: Option<&Fragment>,
        text: Arc<Text>,
        local_timestamp: time::Local,
        lamport_timestamp: time::Lamport,
    ) -> Fragment {
        let new_fragment_id = FragmentId::between(
            &prev_fragment.id,
            next_fragment
                .map(|f| &f.id)
                .unwrap_or(&FragmentId::max_value()),
        );

        let mut split_tree = btree::Tree::new();
        split_tree.push(InsertionSplit {
            extent: text.len(),
            fragment_id: new_fragment_id.clone(),
        });
        self.insertion_splits.insert(local_timestamp, split_tree);

        Fragment::new(
            new_fragment_id,
            Insertion {
                id: local_timestamp,
                parent_id: prev_fragment.insertion.id,
                offset_in_parent: prev_fragment.end_offset,
                text,
                lamport_timestamp,
            },
        )
    }

    pub fn anchor_before_offset(&self, offset: usize) -> Result<Anchor, Error> {
        self.anchor_for_offset(offset, AnchorBias::Left)
    }

    pub fn anchor_after_offset(&self, offset: usize) -> Result<Anchor, Error> {
        self.anchor_for_offset(offset, AnchorBias::Right)
    }

    fn anchor_for_offset(&self, offset: usize, bias: AnchorBias) -> Result<Anchor, Error> {
        let max_offset = self.len();
        if offset > max_offset {
            return Err(Error::OffsetOutOfRange);
        }

        let seek_bias;
        match bias {
            AnchorBias::Left => {
                if offset == 0 {
                    return Ok(Anchor::Start);
                } else {
                    seek_bias = SeekBias::Left;
                }
            }
            AnchorBias::Right => {
                if offset == max_offset {
                    return Ok(Anchor::End);
                } else {
                    seek_bias = SeekBias::Right;
                }
            }
        };

        let mut cursor = self.fragments.cursor();
        cursor.seek(&offset, seek_bias);
        let fragment = cursor.item().unwrap();
        let offset_in_fragment = offset - cursor.start::<usize>();
        let offset_in_insertion = fragment.start_offset + offset_in_fragment;
        let point = cursor.start::<Point>() + &fragment.point_for_offset(offset_in_fragment)?;
        let anchor = Anchor::Middle {
            insertion_id: fragment.insertion.id,
            offset: offset_in_insertion,
            bias,
        };
        self.cache_position(Some(anchor.clone()), offset, point);
        Ok(anchor)
    }

    pub fn anchor_before_point(&self, point: Point) -> Result<Anchor, Error> {
        self.anchor_for_point(point, AnchorBias::Left)
    }

    pub fn anchor_after_point(&self, point: Point) -> Result<Anchor, Error> {
        self.anchor_for_point(point, AnchorBias::Right)
    }

    fn anchor_for_point(&self, point: Point, bias: AnchorBias) -> Result<Anchor, Error> {
        let max_point = self.max_point();
        if point > max_point {
            return Err(Error::OffsetOutOfRange);
        }

        let seek_bias;
        match bias {
            AnchorBias::Left => {
                if point.is_zero() {
                    return Ok(Anchor::Start);
                } else {
                    seek_bias = SeekBias::Left;
                }
            }
            AnchorBias::Right => {
                if point == max_point {
                    return Ok(Anchor::End);
                } else {
                    seek_bias = SeekBias::Right;
                }
            }
        };

        let mut cursor = self.fragments.cursor();
        cursor.seek(&point, seek_bias);
        let fragment = cursor.item().unwrap();
        let offset_in_fragment = fragment.offset_for_point(point - &cursor.start::<Point>())?;
        let offset_in_insertion = fragment.start_offset + offset_in_fragment;
        let anchor = Anchor::Middle {
            insertion_id: fragment.insertion.id,
            offset: offset_in_insertion,
            bias,
        };
        let offset = cursor.start::<usize>() + offset_in_fragment;
        self.cache_position(Some(anchor.clone()), offset, point);
        Ok(anchor)
    }

    pub fn offset_for_anchor(&self, anchor: &Anchor) -> Result<usize, Error> {
        Ok(self.position_for_anchor(anchor)?.0)
    }

    pub fn point_for_anchor(&self, anchor: &Anchor) -> Result<Point, Error> {
        Ok(self.position_for_anchor(anchor)?.1)
    }

    fn position_for_anchor(&self, anchor: &Anchor) -> Result<(usize, Point), Error> {
        match anchor {
            Anchor::Start => Ok((0, Point { row: 0, column: 0 })),
            Anchor::End => Ok((self.len(), self.fragments.extent())),
            Anchor::Middle {
                ref insertion_id,
                offset,
                ref bias,
            } => {
                let cached_position = {
                    let anchor_cache = self.anchor_cache.try_borrow().ok();
                    anchor_cache
                        .as_ref()
                        .and_then(|cache| cache.get(anchor).cloned())
                };

                if let Some(cached_position) = cached_position {
                    Ok(cached_position)
                } else {
                    let seek_bias = match bias {
                        AnchorBias::Left => SeekBias::Left,
                        AnchorBias::Right => SeekBias::Right,
                    };

                    let splits = self
                        .insertion_splits
                        .get(&insertion_id)
                        .ok_or(Error::InvalidAnchor)?;
                    let mut splits_cursor = splits.cursor();
                    splits_cursor.seek(offset, seek_bias);
                    splits_cursor
                        .item()
                        .ok_or(Error::InvalidAnchor)
                        .and_then(|split| {
                            let mut fragments_cursor = self.fragments.cursor();
                            fragments_cursor.seek(&split.fragment_id, SeekBias::Left);
                            fragments_cursor
                                .item()
                                .ok_or(Error::InvalidAnchor)
                                .and_then(|fragment| {
                                    let overshoot = if fragment.is_visible() {
                                        offset - fragment.start_offset
                                    } else {
                                        0
                                    };
                                    let offset = fragments_cursor.start::<usize>() + overshoot;
                                    let point = fragments_cursor.start::<Point>()
                                        + &fragment.point_for_offset(overshoot)?;
                                    self.cache_position(Some(anchor.clone()), offset, point);
                                    Ok((offset, point))
                                })
                        })
                }
            }
        }
    }

    fn offset_for_point(&self, point: Point) -> Result<usize, Error> {
        let cached_offset = {
            let offset_cache = self.offset_cache.try_borrow().ok();
            offset_cache
                .as_ref()
                .and_then(|cache| cache.get(&point).cloned())
        };

        if let Some(cached_offset) = cached_offset {
            Ok(cached_offset)
        } else {
            let mut fragments_cursor = self.fragments.cursor();
            fragments_cursor.seek(&point, SeekBias::Left);
            fragments_cursor
                .item()
                .ok_or(Error::OffsetOutOfRange)
                .map(|fragment| {
                    let overshoot = fragment
                        .offset_for_point(point - &fragments_cursor.start::<Point>())
                        .unwrap();
                    let offset = &fragments_cursor.start::<usize>() + &overshoot;
                    self.cache_position(None, offset, point);
                    offset
                })
        }
    }

    pub fn cmp_anchors(&self, a: &Anchor, b: &Anchor) -> Result<Ordering, Error> {
        let a_offset = self.offset_for_anchor(a)?;
        let b_offset = self.offset_for_anchor(b)?;
        Ok(a_offset.cmp(&b_offset))
    }

    fn cache_position(&self, anchor: Option<Anchor>, offset: usize, point: Point) {
        anchor.map(|anchor| {
            if let Ok(mut anchor_cache) = self.anchor_cache.try_borrow_mut() {
                anchor_cache.insert(anchor, (offset, point));
            }
        });

        if let Ok(mut offset_cache) = self.offset_cache.try_borrow_mut() {
            offset_cache.insert(point, offset);
        }
    }
}

impl Point {
    pub fn new(row: u32, column: u32) -> Self {
        Point { row, column }
    }

    pub fn zero() -> Self {
        Point::new(0, 0)
    }

    pub fn is_zero(&self) -> bool {
        self.row == 0 && self.column == 0
    }
}

impl btree::Dimension<FragmentSummary> for Point {
    fn from_summary(summary: &FragmentSummary) -> Self {
        summary.extent_2d
    }
}

impl<'a> Add<&'a Self> for Point {
    type Output = Point;

    fn add(self, other: &'a Self) -> Self::Output {
        if other.row == 0 {
            Point::new(self.row, self.column + other.column)
        } else {
            Point::new(self.row + other.row, other.column)
        }
    }
}

impl<'a> Sub<&'a Self> for Point {
    type Output = Point;

    fn sub(self, other: &'a Self) -> Self::Output {
        debug_assert!(*other <= self);

        if self.row == other.row {
            Point::new(0, self.column - other.column)
        } else {
            Point::new(self.row - other.row, self.column)
        }
    }
}

impl<'a> AddAssign<&'a Self> for Point {
    fn add_assign(&mut self, other: &'a Self) {
        if other.row == 0 {
            self.column += other.column;
        } else {
            self.row += other.row;
            self.column = other.column;
        }
    }
}

impl PartialOrd for Point {
    fn partial_cmp(&self, other: &Point) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Point {
    #[cfg(target_pointer_width = "64")]
    fn cmp(&self, other: &Point) -> Ordering {
        let a = (self.row as usize) << 32 | self.column as usize;
        let b = (other.row as usize) << 32 | other.column as usize;
        a.cmp(&b)
    }

    #[cfg(target_pointer_width = "32")]
    fn cmp(&self, other: &Point) -> Ordering {
        match self.row.cmp(&other.row) {
            Ordering::Equal => self.column.cmp(&other.column),
            comparison @ _ => comparison,
        }
    }
}

impl Anchor {
    fn to_flatbuf<'fbb>(
        &self,
        builder: &mut FlatBufferBuilder<'fbb>,
    ) -> WIPOffset<serialization::buffer::Anchor<'fbb>> {
        match self {
            Anchor::Start => serialization::buffer::Anchor::create(
                builder,
                &serialization::buffer::AnchorArgs {
                    variant: serialization::buffer::AnchorVariant::Start,
                    ..serialization::buffer::AnchorArgs::default()
                },
            ),
            Anchor::End => serialization::buffer::Anchor::create(
                builder,
                &serialization::buffer::AnchorArgs {
                    variant: serialization::buffer::AnchorVariant::End,
                    ..serialization::buffer::AnchorArgs::default()
                },
            ),
            Anchor::Middle {
                insertion_id,
                offset,
                bias,
            } => serialization::buffer::Anchor::create(
                builder,
                &serialization::buffer::AnchorArgs {
                    variant: serialization::buffer::AnchorVariant::Middle,
                    insertion_id: Some(&insertion_id.to_flatbuf()),
                    offset: *offset as u64,
                    bias: bias.to_flatbuf(),
                },
            ),
        }
    }

    fn from_flatbuf<'fbb>(
        message: &serialization::buffer::Anchor<'fbb>,
    ) -> Result<Self, crate::Error> {
        match message.variant() {
            serialization::buffer::AnchorVariant::Start => Ok(Anchor::Start),
            serialization::buffer::AnchorVariant::End => Ok(Anchor::End),
            serialization::buffer::AnchorVariant::Middle => Ok(Anchor::Middle {
                insertion_id: time::Local::from_flatbuf(
                    message
                        .insertion_id()
                        .ok_or(crate::Error::DeserializeError)?,
                ),
                offset: message.offset() as usize,
                bias: AnchorBias::from_flatbuf(message.bias()),
            }),
        }
    }
}

impl AnchorBias {
    fn to_flatbuf(&self) -> serialization::buffer::AnchorBias {
        match self {
            AnchorBias::Left => serialization::buffer::AnchorBias::Left,
            AnchorBias::Right => serialization::buffer::AnchorBias::Right,
        }
    }

    fn from_flatbuf(message: serialization::buffer::AnchorBias) -> Self {
        match message {
            serialization::buffer::AnchorBias::Left => AnchorBias::Left,
            serialization::buffer::AnchorBias::Right => AnchorBias::Right,
        }
    }
}

impl Iter {
    fn new(buffer: &Buffer) -> Self {
        let mut fragment_cursor = buffer.fragments.cursor();
        fragment_cursor.seek(&0, SeekBias::Right);
        Self {
            fragment_cursor,
            fragment_offset: 0,
            reversed: false,
        }
    }

    fn at_point(buffer: &Buffer, point: Point) -> Self {
        let mut fragment_cursor = buffer.fragments.cursor();
        fragment_cursor.seek(&point, SeekBias::Right);
        let fragment_offset = if let Some(fragment) = fragment_cursor.item() {
            let point_in_fragment = point - &fragment_cursor.start::<Point>();
            fragment.offset_for_point(point_in_fragment).unwrap()
        } else {
            0
        };

        Self {
            fragment_cursor,
            fragment_offset,
            reversed: false,
        }
    }

    pub fn rev(mut self) -> Iter {
        self.reversed = true;
        self
    }

    pub fn into_string(self) -> String {
        String::from_utf16_lossy(&self.collect::<Vec<u16>>())
    }
}

impl Iterator for Iter {
    type Item = u16;

    fn next(&mut self) -> Option<Self::Item> {
        if self.reversed {
            if let Some(fragment) = self.fragment_cursor.item() {
                if self.fragment_offset > 0 {
                    self.fragment_offset -= 1;
                    if let Some(c) = fragment.code_unit(self.fragment_offset) {
                        return Some(c);
                    }
                }
            }

            loop {
                self.fragment_cursor.prev();
                if let Some(fragment) = self.fragment_cursor.item() {
                    if fragment.len() > 0 {
                        self.fragment_offset = fragment.len() - 1;
                        return fragment.code_unit(self.fragment_offset);
                    }
                } else {
                    break;
                }
            }

            None
        } else {
            if let Some(fragment) = self.fragment_cursor.item() {
                if let Some(c) = fragment.code_unit(self.fragment_offset) {
                    self.fragment_offset += 1;
                    return Some(c);
                }
            }

            loop {
                self.fragment_cursor.next();
                if let Some(fragment) = self.fragment_cursor.item() {
                    if let Some(c) = fragment.code_unit(0) {
                        self.fragment_offset = 1;
                        return Some(c);
                    }
                } else {
                    break;
                }
            }

            None
        }
    }
}

impl<F: Fn(&FragmentSummary) -> bool> Iterator for ChangesIter<F> {
    type Item = Change;

    fn next(&mut self) -> Option<Self::Item> {
        let mut change: Option<Change> = None;

        while let Some(fragment) = self.cursor.item() {
            let position = self.cursor.start();
            if !fragment.was_visible(&self.since) && fragment.is_visible() {
                if let Some(ref mut change) = change {
                    if change.range.start + &change.new_extent == position {
                        change.code_units.extend(fragment.code_units());
                        change.new_extent += &fragment.extent_2d();
                    } else {
                        break;
                    }
                } else {
                    change = Some(Change {
                        range: position..position,
                        code_units: Vec::from(fragment.code_units()),
                        new_extent: fragment.extent_2d(),
                    });
                }
            } else if fragment.was_visible(&self.since) && !fragment.is_visible() {
                if let Some(ref mut change) = change {
                    if change.range.start + &change.new_extent == position {
                        change.range.end += &fragment.extent_2d();
                    } else {
                        break;
                    }
                } else {
                    change = Some(Change {
                        range: position..position + &fragment.extent_2d(),
                        code_units: Vec::new(),
                        new_extent: Point::zero(),
                    });
                }
            }

            self.cursor.next();
        }

        change
    }
}

pub fn diff(a: &str, b: &str) -> impl Iterator<Item = Change> {
    DiffIter {
        position: Point::zero(),
        diff: Changeset::new(a, b, "").diffs.into_iter(),
    }
}

impl Iterator for DiffIter {
    type Item = Change;

    fn next(&mut self) -> Option<Self::Item> {
        let mut change: Option<Change> = None;

        while let Some(diff) = self.diff.next() {
            let code_units;
            let extent;
            match &diff {
                Difference::Same(text) | Difference::Rem(text) | Difference::Add(text) => {
                    code_units = text.encode_utf16().collect::<Vec<_>>();

                    let mut rows = 0;
                    let mut last_row_len = 0;
                    for ch in &code_units {
                        if *ch == b'\n' as u16 {
                            rows += 1;
                            last_row_len = 0;
                        } else {
                            last_row_len += 1;
                        }
                    }
                    extent = Point::new(rows, last_row_len);
                }
            }

            match diff {
                Difference::Same(_) => {
                    self.position += &extent;
                    if change.is_some() {
                        break;
                    }
                }
                Difference::Rem(_) => {
                    if let Some(change) = change.as_mut() {
                        change.range.end += &extent;
                    } else {
                        change = Some(Change {
                            range: self.position..self.position + &extent,
                            code_units: Vec::new(),
                            new_extent: Point::zero(),
                        });
                    }
                }
                Difference::Add(_) => {
                    if let Some(change) = change.as_mut() {
                        change.code_units.extend(code_units);
                        change.new_extent += &extent;
                    } else {
                        change = Some(Change {
                            range: self.position..self.position,
                            code_units,
                            new_extent: extent,
                        });
                    }
                    self.position += &extent;
                }
            }
        }

        change
    }
}

impl Selection {
    pub fn head(&self) -> &Anchor {
        if self.reversed {
            &self.start
        } else {
            &self.end
        }
    }

    pub fn set_head<S>(&mut self, buffer: &Buffer, cursor: Anchor) {
        if buffer.cmp_anchors(&cursor, self.tail()).unwrap() < Ordering::Equal {
            if !self.reversed {
                mem::swap(&mut self.start, &mut self.end);
                self.reversed = true;
            }
            self.start = cursor;
        } else {
            if self.reversed {
                mem::swap(&mut self.start, &mut self.end);
                self.reversed = false;
            }
            self.end = cursor;
        }
    }

    pub fn tail(&self) -> &Anchor {
        if self.reversed {
            &self.end
        } else {
            &self.start
        }
    }

    pub fn is_empty(&self, buffer: &Buffer) -> bool {
        buffer.cmp_anchors(&self.start, &self.end).unwrap() == Ordering::Equal
    }

    pub fn anchor_range(&self) -> Range<Anchor> {
        self.start.clone()..self.end.clone()
    }

    fn to_flatbuf<'fbb>(
        &self,
        builder: &mut FlatBufferBuilder<'fbb>,
    ) -> WIPOffset<serialization::buffer::Selection<'fbb>> {
        let start = Some(self.start.to_flatbuf(builder));
        let end = Some(self.end.to_flatbuf(builder));

        serialization::buffer::Selection::create(
            builder,
            &serialization::buffer::SelectionArgs {
                start,
                end,
                reversed: self.reversed,
            },
        )
    }

    fn from_flatbuf<'fbb>(
        message: serialization::buffer::Selection<'fbb>,
    ) -> Result<Self, crate::Error> {
        Ok(Self {
            start: Anchor::from_flatbuf(&message.start().ok_or(crate::Error::DeserializeError)?)?,
            end: Anchor::from_flatbuf(&message.end().ok_or(crate::Error::DeserializeError)?)?,
            reversed: message.reversed(),
        })
    }
}

impl Text {
    fn new(code_units: Vec<u16>) -> Self {
        fn build_tree(index: usize, line_lengths: &[u32], mut tree: &mut [LineNode]) {
            if line_lengths.is_empty() {
                return;
            }

            let mid = if line_lengths.len() == 1 {
                0
            } else {
                let depth = log2_fast(line_lengths.len());
                let max_elements = (1 << (depth)) - 1;
                let right_subtree_elements = 1 << (depth - 1);
                cmp::min(line_lengths.len() - right_subtree_elements, max_elements)
            };
            let len = line_lengths[mid];
            let lower = &line_lengths[0..mid];
            let upper = &line_lengths[mid + 1..];

            let left_child_index = index * 2 + 1;
            let right_child_index = index * 2 + 2;
            build_tree(left_child_index, lower, &mut tree);
            build_tree(right_child_index, upper, &mut tree);
            tree[index] = {
                let mut left_child_longest_row = 0;
                let mut left_child_longest_row_len = 0;
                let mut left_child_offset = 0;
                let mut left_child_rows = 0;
                if let Some(left_child) = tree.get(left_child_index) {
                    left_child_longest_row = left_child.longest_row;
                    left_child_longest_row_len = left_child.longest_row_len;
                    left_child_offset = left_child.offset;
                    left_child_rows = left_child.rows;
                }
                let mut right_child_longest_row = 0;
                let mut right_child_longest_row_len = 0;
                let mut right_child_offset = 0;
                let mut right_child_rows = 0;
                if let Some(right_child) = tree.get(right_child_index) {
                    right_child_longest_row = right_child.longest_row;
                    right_child_longest_row_len = right_child.longest_row_len;
                    right_child_offset = right_child.offset;
                    right_child_rows = right_child.rows;
                }

                let mut longest_row = 0;
                let mut longest_row_len = 0;
                if left_child_longest_row_len > longest_row_len {
                    longest_row = left_child_longest_row;
                    longest_row_len = left_child_longest_row_len;
                }
                if len > longest_row_len {
                    longest_row = left_child_rows;
                    longest_row_len = len;
                }
                if right_child_longest_row_len > longest_row_len {
                    longest_row = left_child_rows + right_child_longest_row + 1;
                    longest_row_len = right_child_longest_row_len;
                }

                LineNode {
                    len,
                    longest_row,
                    longest_row_len,
                    offset: left_child_offset + len as usize + right_child_offset + 1,
                    rows: left_child_rows + right_child_rows + 1,
                }
            };
        }

        let mut line_lengths = Vec::new();
        let mut prev_offset = 0;
        for (offset, code_unit) in code_units.iter().enumerate() {
            if code_unit == &u16::from(b'\n') {
                line_lengths.push((offset - prev_offset) as u32);
                prev_offset = offset + 1;
            }
        }
        line_lengths.push((code_units.len() - prev_offset) as u32);

        let mut nodes = Vec::new();
        nodes.resize(
            line_lengths.len(),
            LineNode {
                len: 0,
                longest_row_len: 0,
                longest_row: 0,
                offset: 0,
                rows: 0,
            },
        );
        build_tree(0, &line_lengths, &mut nodes);

        Self { code_units, nodes }
    }

    fn len(&self) -> usize {
        self.code_units.len()
    }

    fn longest_row_in_range(&self, target_range: Range<usize>) -> Result<(u32, u32), Error> {
        let mut longest_row = 0;
        let mut longest_row_len = 0;

        self.search(|probe| {
            if target_range.start <= probe.offset_range.end
                && probe.right_ancestor_start_offset <= target_range.end
            {
                if let Some(right_child) = probe.right_child {
                    if right_child.longest_row_len >= longest_row_len {
                        longest_row = probe.row + 1 + right_child.longest_row;
                        longest_row_len = right_child.longest_row_len;
                    }
                }
            }

            if target_range.start < probe.offset_range.start {
                if probe.offset_range.end < target_range.end && probe.node.len >= longest_row_len {
                    longest_row = probe.row;
                    longest_row_len = probe.node.len;
                }

                Ordering::Less
            } else if target_range.start > probe.offset_range.end {
                Ordering::Greater
            } else {
                let node_end = cmp::min(probe.offset_range.end, target_range.end);
                let node_len = (node_end - target_range.start) as u32;
                if node_len >= longest_row_len {
                    longest_row = probe.row;
                    longest_row_len = node_len;
                }
                Ordering::Equal
            }
        })
        .ok_or(Error::OffsetOutOfRange)?;

        self.search(|probe| {
            if target_range.end >= probe.offset_range.start
                && probe.left_ancestor_end_offset >= target_range.start
            {
                if let Some(left_child) = probe.left_child {
                    if left_child.longest_row_len > longest_row_len {
                        let left_ancestor_row = probe.row - left_child.rows;
                        longest_row = left_ancestor_row + left_child.longest_row;
                        longest_row_len = left_child.longest_row_len;
                    }
                }
            }

            if target_range.end < probe.offset_range.start {
                Ordering::Less
            } else if target_range.end > probe.offset_range.end {
                if target_range.start < probe.offset_range.start && probe.node.len > longest_row_len
                {
                    longest_row = probe.row;
                    longest_row_len = probe.node.len;
                }

                Ordering::Greater
            } else {
                let node_start = cmp::max(target_range.start, probe.offset_range.start);
                let node_len = (target_range.end - node_start) as u32;
                if node_len > longest_row_len {
                    longest_row = probe.row;
                    longest_row_len = node_len;
                }
                Ordering::Equal
            }
        })
        .ok_or(Error::OffsetOutOfRange)?;

        Ok((longest_row, longest_row_len))
    }

    fn point_for_offset(&self, offset: usize) -> Result<Point, Error> {
        let search_result = self.search(|probe| {
            if offset < probe.offset_range.start {
                Ordering::Less
            } else if offset > probe.offset_range.end {
                Ordering::Greater
            } else {
                Ordering::Equal
            }
        });
        if let Some((offset_range, row, _)) = search_result {
            Ok(Point::new(row, (offset - offset_range.start) as u32))
        } else {
            Err(Error::OffsetOutOfRange)
        }
    }

    fn offset_for_point(&self, point: Point) -> Result<usize, Error> {
        if let Some((offset_range, _, node)) = self.search(|probe| point.row.cmp(&probe.row)) {
            if point.column <= node.len {
                Ok(offset_range.start + point.column as usize)
            } else {
                Err(Error::OffsetOutOfRange)
            }
        } else {
            Err(Error::OffsetOutOfRange)
        }
    }

    fn search<F>(&self, mut f: F) -> Option<(Range<usize>, u32, &LineNode)>
    where
        F: FnMut(LineNodeProbe) -> Ordering,
    {
        let mut left_ancestor_end_offset = 0;
        let mut left_ancestor_row = 0;
        let mut right_ancestor_start_offset = self.nodes[0].offset;
        let mut cur_node_index = 0;
        while let Some(cur_node) = self.nodes.get(cur_node_index) {
            let left_child = self.nodes.get(cur_node_index * 2 + 1);
            let right_child = self.nodes.get(cur_node_index * 2 + 2);
            let cur_offset_range = {
                let start = left_ancestor_end_offset + left_child.map_or(0, |node| node.offset);
                let end = start + cur_node.len as usize;
                start..end
            };
            let cur_row = left_ancestor_row + left_child.map_or(0, |node| node.rows);
            match f(LineNodeProbe {
                offset_range: &cur_offset_range,
                row: cur_row,
                left_ancestor_end_offset,
                right_ancestor_start_offset,
                node: cur_node,
                left_child,
                right_child,
            }) {
                Ordering::Less => {
                    cur_node_index = cur_node_index * 2 + 1;
                    right_ancestor_start_offset = cur_offset_range.start;
                }
                Ordering::Equal => return Some((cur_offset_range, cur_row, cur_node)),
                Ordering::Greater => {
                    cur_node_index = cur_node_index * 2 + 2;
                    left_ancestor_end_offset = cur_offset_range.end + 1;
                    left_ancestor_row = cur_row + 1;
                }
            }
        }
        None
    }
}

impl<'a> From<&'a str> for Text {
    fn from(s: &'a str) -> Self {
        Self::new(s.encode_utf16().collect())
    }
}

impl From<String> for Text {
    fn from(s: String) -> Self {
        Self::from(s.as_str())
    }
}

impl<'a> From<Vec<u16>> for Text {
    fn from(s: Vec<u16>) -> Self {
        Self::new(s)
    }
}

#[inline(always)]
fn log2_fast(x: usize) -> usize {
    8 * mem::size_of::<usize>() - (x.leading_zeros() as usize) - 1
}

lazy_static! {
    static ref FRAGMENT_ID_MIN_VALUE: FragmentId = FragmentId(Arc::new(vec![0 as u16]));
    static ref FRAGMENT_ID_MAX_VALUE: FragmentId = FragmentId(Arc::new(vec![u16::max_value()]));
}

impl FragmentId {
    fn min_value() -> Self {
        FRAGMENT_ID_MIN_VALUE.clone()
    }

    fn max_value() -> Self {
        FRAGMENT_ID_MAX_VALUE.clone()
    }

    fn between(left: &Self, right: &Self) -> Self {
        Self::between_with_max(left, right, u16::max_value())
    }

    fn between_with_max(left: &Self, right: &Self, max_value: u16) -> Self {
        let mut new_entries = Vec::new();

        let left_entries = left.0.iter().cloned().chain(iter::repeat(0));
        let right_entries = right.0.iter().cloned().chain(iter::repeat(max_value));
        for (l, r) in left_entries.zip(right_entries) {
            let interval = r - l;
            if interval > 1 {
                new_entries.push(l + interval / 2);
                break;
            } else {
                new_entries.push(l);
            }
        }

        FragmentId(Arc::new(new_entries))
    }
}

impl btree::Dimension<FragmentSummary> for FragmentId {
    fn from_summary(summary: &FragmentSummary) -> Self {
        summary.max_fragment_id.clone()
    }
}

impl<'a> Add<&'a Self> for FragmentId {
    type Output = FragmentId;

    fn add(self, other: &'a Self) -> Self::Output {
        debug_assert!(self <= *other);
        other.clone()
    }
}

impl<'a> AddAssign<&'a Self> for FragmentId {
    fn add_assign(&mut self, other: &'a Self) {
        debug_assert!(*self <= *other);
        *self = other.clone();
    }
}

impl Fragment {
    fn new(id: FragmentId, insertion: Insertion) -> Self {
        let end_offset = insertion.text.len();
        Self {
            id,
            insertion,
            start_offset: 0,
            end_offset,
            deletions: HashSet::new(),
        }
    }

    fn code_unit(&self, offset: usize) -> Option<u16> {
        if offset < self.len() {
            Some(self.insertion.text.code_units[self.start_offset + offset].clone())
        } else {
            None
        }
    }

    fn code_units(&self) -> &[u16] {
        &self.insertion.text.code_units[self.start_offset..self.end_offset]
    }

    fn len(&self) -> usize {
        if self.is_visible() {
            self.extent()
        } else {
            0
        }
    }

    fn extent(&self) -> usize {
        self.end_offset - self.start_offset
    }

    fn extent_2d(&self) -> Point {
        self.point_for_offset(self.extent()).unwrap()
    }

    fn is_visible(&self) -> bool {
        self.deletions.is_empty()
    }

    fn was_visible(&self, version: &time::Global) -> bool {
        version.observed(self.insertion.id) && self.deletions.iter().all(|d| !version.observed(*d))
    }

    fn point_for_offset(&self, offset: usize) -> Result<Point, Error> {
        let text = &self.insertion.text;
        let offset_in_insertion = self.start_offset + offset;
        Ok(text.point_for_offset(offset_in_insertion)?
            - &text.point_for_offset(self.start_offset)?)
    }

    fn offset_for_point(&self, point: Point) -> Result<usize, Error> {
        let text = &self.insertion.text;
        let point_in_insertion = text.point_for_offset(self.start_offset)? + &point;
        Ok(text.offset_for_point(point_in_insertion)? - self.start_offset)
    }
}

impl btree::Item for Fragment {
    type Summary = FragmentSummary;

    fn summarize(&self) -> Self::Summary {
        let mut max_version = time::Global::new();
        max_version.observe(self.insertion.id);
        for deletion in &self.deletions {
            max_version.observe(*deletion);
        }

        if self.is_visible() {
            let fragment_2d_start = self
                .insertion
                .text
                .point_for_offset(self.start_offset)
                .unwrap();
            let fragment_2d_end = self
                .insertion
                .text
                .point_for_offset(self.end_offset)
                .unwrap();

            let first_row_len = if fragment_2d_start.row == fragment_2d_end.row {
                self.extent() as u32
            } else {
                self.offset_for_point(Point::new(1, 0)).unwrap() as u32 - 1
            };
            let (longest_row, longest_row_len) = self
                .insertion
                .text
                .longest_row_in_range(self.start_offset as usize..self.end_offset as usize)
                .unwrap();
            FragmentSummary {
                extent: self.len(),
                extent_2d: fragment_2d_end - &fragment_2d_start,
                max_fragment_id: self.id.clone(),
                first_row_len,
                longest_row: longest_row - fragment_2d_start.row,
                longest_row_len,
                max_version,
            }
        } else {
            FragmentSummary {
                extent: 0,
                extent_2d: Point { row: 0, column: 0 },
                max_fragment_id: self.id.clone(),
                first_row_len: 0,
                longest_row: 0,
                longest_row_len: 0,
                max_version,
            }
        }
    }
}

impl<'a> AddAssign<&'a FragmentSummary> for FragmentSummary {
    fn add_assign(&mut self, other: &Self) {
        let last_row_len = self.extent_2d.column + other.first_row_len;
        if last_row_len > self.longest_row_len {
            self.longest_row = self.extent_2d.row;
            self.longest_row_len = last_row_len;
        }
        if other.longest_row_len > self.longest_row_len {
            self.longest_row = self.extent_2d.row + other.longest_row;
            self.longest_row_len = other.longest_row_len;
        }
        if self.extent_2d.row == 0 {
            self.first_row_len += other.first_row_len;
        }

        self.extent += other.extent;
        self.extent_2d += &other.extent_2d;
        debug_assert!(self.max_fragment_id <= other.max_fragment_id);
        self.max_fragment_id = other.max_fragment_id.clone();
        self.max_version.observe_all(&other.max_version);
    }
}

impl Default for FragmentSummary {
    fn default() -> Self {
        FragmentSummary {
            extent: 0,
            extent_2d: Point { row: 0, column: 0 },
            max_fragment_id: FragmentId::min_value(),
            first_row_len: 0,
            longest_row: 0,
            longest_row_len: 0,
            max_version: time::Global::new(),
        }
    }
}

impl btree::Dimension<FragmentSummary> for usize {
    fn from_summary(summary: &FragmentSummary) -> Self {
        summary.extent
    }
}

impl btree::Item for InsertionSplit {
    type Summary = InsertionSplitSummary;

    fn summarize(&self) -> Self::Summary {
        InsertionSplitSummary {
            extent: self.extent,
        }
    }
}

impl<'a> AddAssign<&'a InsertionSplitSummary> for InsertionSplitSummary {
    fn add_assign(&mut self, other: &Self) {
        self.extent += other.extent;
    }
}

impl Default for InsertionSplitSummary {
    fn default() -> Self {
        InsertionSplitSummary { extent: 0 }
    }
}

impl btree::Dimension<InsertionSplitSummary> for usize {
    fn from_summary(summary: &InsertionSplitSummary) -> Self {
        summary.extent
    }
}

impl Operation {
    fn replica_id(&self) -> ReplicaId {
        match self {
            Operation::Edit {
                local_timestamp, ..
            } => local_timestamp.replica_id,
            Operation::UpdateSelections {
                local_timestamp, ..
            } => local_timestamp.replica_id,
        }
    }

    pub fn to_flatbuf<'fbb>(
        &self,
        builder: &mut FlatBufferBuilder<'fbb>,
    ) -> WIPOffset<serialization::buffer::OperationEnvelope<'fbb>> {
        let operation_type;
        let operation;
        match self {
            Operation::Edit {
                start_id,
                start_offset,
                end_id,
                end_offset,
                version_in_range,
                new_text,
                local_timestamp,
                lamport_timestamp,
            } => {
                let new_text = new_text.as_ref().map(|new_text| {
                    builder.create_string(String::from_utf16_lossy(&new_text.code_units).as_str())
                });
                let version_in_range = Some(version_in_range.to_flatbuf(builder));
                operation_type = serialization::buffer::Operation::Edit;
                operation = serialization::buffer::Edit::create(
                    builder,
                    &serialization::buffer::EditArgs {
                        start_id: Some(&start_id.to_flatbuf()),
                        start_offset: *start_offset as u64,
                        end_id: Some(&end_id.to_flatbuf()),
                        end_offset: *end_offset as u64,
                        version_in_range,
                        new_text,
                        local_timestamp: Some(&local_timestamp.to_flatbuf()),
                        lamport_timestamp: Some(&lamport_timestamp.to_flatbuf()),
                    },
                )
                .as_union_value();
            }
            Operation::UpdateSelections {
                set_id,
                selections,
                local_timestamp,
                lamport_timestamp,
            } => {
                operation_type = serialization::buffer::Operation::UpdateSelections;
                let selections = selections.as_ref().map(|selections| {
                    let selection_flatbufs = &selections
                        .iter()
                        .map(|s| s.to_flatbuf(builder))
                        .collect::<Vec<_>>();
                    builder.create_vector(selection_flatbufs)
                });
                operation = serialization::buffer::UpdateSelections::create(
                    builder,
                    &serialization::buffer::UpdateSelectionsArgs {
                        set_id: Some(&set_id.to_flatbuf()),
                        selections,
                        local_timestamp: Some(&local_timestamp.to_flatbuf()),
                        lamport_timestamp: Some(&lamport_timestamp.to_flatbuf()),
                    },
                )
                .as_union_value();
            }
        }

        serialization::buffer::OperationEnvelope::create(
            builder,
            &serialization::buffer::OperationEnvelopeArgs {
                operation_type,
                operation: Some(operation),
            },
        )
    }

    pub fn from_flatbuf<'fbb>(
        message: &serialization::buffer::OperationEnvelope<'fbb>,
    ) -> Result<Option<Self>, crate::Error> {
        match message.operation_type() {
            serialization::buffer::Operation::Edit => {
                let message = serialization::buffer::Edit::init_from_table(
                    message.operation().ok_or(crate::Error::DeserializeError)?,
                );
                Ok(Some(Operation::Edit {
                    start_id: time::Local::from_flatbuf(
                        message.start_id().ok_or(crate::Error::DeserializeError)?,
                    ),
                    start_offset: message.start_offset() as usize,
                    end_id: time::Local::from_flatbuf(
                        message.end_id().ok_or(crate::Error::DeserializeError)?,
                    ),
                    end_offset: message.end_offset() as usize,
                    version_in_range: time::Global::from_flatbuf(
                        message
                            .version_in_range()
                            .ok_or(crate::Error::DeserializeError)?,
                    )?,
                    new_text: message.new_text().map(|new_text| Arc::new(new_text.into())),
                    local_timestamp: time::Local::from_flatbuf(
                        message
                            .local_timestamp()
                            .ok_or(crate::Error::DeserializeError)?,
                    ),
                    lamport_timestamp: time::Lamport::from_flatbuf(
                        message
                            .lamport_timestamp()
                            .ok_or(crate::Error::DeserializeError)?,
                    ),
                }))
            }
            serialization::buffer::Operation::UpdateSelections => {
                let message = serialization::buffer::UpdateSelections::init_from_table(
                    message.operation().ok_or(crate::Error::DeserializeError)?,
                );

                let selections = if let Some(flatbufs) = message.selections() {
                    let mut selections = Vec::with_capacity(flatbufs.len());
                    for i in 0..flatbufs.len() {
                        selections.push(Selection::from_flatbuf(flatbufs.get(i))?);
                    }
                    Some(selections)
                } else {
                    None
                };

                Ok(Some(Operation::UpdateSelections {
                    set_id: time::Local::from_flatbuf(
                        message.set_id().ok_or(crate::Error::DeserializeError)?,
                    ),
                    selections,
                    local_timestamp: time::Local::from_flatbuf(
                        message
                            .local_timestamp()
                            .ok_or(crate::Error::DeserializeError)?,
                    ),
                    lamport_timestamp: time::Lamport::from_flatbuf(
                        message
                            .lamport_timestamp()
                            .ok_or(crate::Error::DeserializeError)?,
                    ),
                }))
            }
            serialization::buffer::Operation::NONE => Ok(None),
        }
    }
}

impl operation_queue::Operation for Operation {
    fn timestamp(&self) -> time::Lamport {
        match self {
            Operation::Edit {
                lamport_timestamp, ..
            } => *lamport_timestamp,
            Operation::UpdateSelections {
                lamport_timestamp, ..
            } => *lamport_timestamp,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::{Rng, SeedableRng, StdRng};
    use uuid::Uuid;

    #[test]
    fn test_edit() {
        let replica_id = Uuid::from_u128(1);
        let mut local_clock = time::Local::new(replica_id);
        let mut lamport_clock = time::Lamport::new(replica_id);
        let mut buffer = Buffer::new("abc");
        assert_eq!(buffer.to_string(), "abc");
        buffer.edit(vec![3..3], "def", &mut local_clock, &mut lamport_clock);
        assert_eq!(buffer.to_string(), "abcdef");
        buffer.edit(vec![0..0], "ghi", &mut local_clock, &mut lamport_clock);
        assert_eq!(buffer.to_string(), "ghiabcdef");
        buffer.edit(vec![5..5], "jkl", &mut local_clock, &mut lamport_clock);
        assert_eq!(buffer.to_string(), "ghiabjklcdef");
        buffer.edit(vec![6..7], "", &mut local_clock, &mut lamport_clock);
        assert_eq!(buffer.to_string(), "ghiabjlcdef");
        buffer.edit(vec![4..9], "mno", &mut local_clock, &mut lamport_clock);
        assert_eq!(buffer.to_string(), "ghiamnoef");
    }

    #[test]
    fn test_random_edits() {
        for seed in 0..100 {
            println!("{:?}", seed);
            let mut rng = StdRng::from_seed(&[seed]);

            let mut reference_string = RandomCharIter(rng)
                .take(rng.gen_range(0, 10))
                .collect::<String>();
            let mut buffer = Buffer::new(reference_string.as_str());
            let mut buffer_versions = Vec::new();
            let replica_id = Uuid::from_u128(1);
            let mut local_clock = time::Local::new(replica_id);
            let mut lamport_clock = time::Lamport::new(replica_id);

            for _i in 0..10 {
                let (old_ranges, new_text, _) =
                    buffer.randomly_mutate(&mut rng, &mut local_clock, &mut lamport_clock);
                for old_range in old_ranges.iter().rev() {
                    reference_string = [
                        &reference_string[0..old_range.start],
                        new_text.as_str(),
                        &reference_string[old_range.end..],
                    ]
                    .concat();
                }
                assert_eq!(buffer.to_string(), reference_string);

                if rng.gen_weighted_bool(3) {
                    buffer_versions.push(buffer.clone());
                }
            }

            for mut old_buffer in buffer_versions {
                for change in buffer.changes_since(&old_buffer.version) {
                    old_buffer.edit_2d(
                        Some(change.range),
                        Text::new(change.code_units),
                        &mut local_clock,
                        &mut lamport_clock,
                    );
                }
                assert_eq!(old_buffer.to_string(), buffer.to_string());
            }
        }
    }

    #[test]
    fn test_len_for_row() {
        let mut buffer = Buffer::new("");
        let replica_id = Uuid::from_u128(1);
        let mut local_clock = time::Local::new(replica_id);
        let mut lamport_clock = time::Lamport::new(replica_id);
        buffer.edit(
            vec![0..0],
            "abcd\nefg\nhij",
            &mut local_clock,
            &mut lamport_clock,
        );
        buffer.edit(
            vec![12..12],
            "kl\nmno",
            &mut local_clock,
            &mut lamport_clock,
        );
        buffer.edit(
            vec![18..18],
            "\npqrs\n",
            &mut local_clock,
            &mut lamport_clock,
        );
        buffer.edit(vec![18..21], "\nPQ", &mut local_clock, &mut lamport_clock);

        assert_eq!(buffer.len_for_row(0), Ok(4));
        assert_eq!(buffer.len_for_row(1), Ok(3));
        assert_eq!(buffer.len_for_row(2), Ok(5));
        assert_eq!(buffer.len_for_row(3), Ok(3));
        assert_eq!(buffer.len_for_row(4), Ok(4));
        assert_eq!(buffer.len_for_row(5), Ok(0));
        assert_eq!(buffer.len_for_row(6), Err(Error::OffsetOutOfRange));
    }

    #[test]
    fn test_longest_row() {
        let mut buffer = Buffer::new("");
        let replica_id = Uuid::from_u128(1);
        let mut local_clock = time::Local::new(replica_id);
        let mut lamport_clock = time::Lamport::new(replica_id);
        assert_eq!(buffer.longest_row(), 0);
        buffer.edit(
            vec![0..0],
            "abcd\nefg\nhij",
            &mut local_clock,
            &mut lamport_clock,
        );
        assert_eq!(buffer.longest_row(), 0);
        buffer.edit(
            vec![12..12],
            "kl\nmno",
            &mut local_clock,
            &mut lamport_clock,
        );
        assert_eq!(buffer.longest_row(), 2);
        buffer.edit(vec![18..18], "\npqrs", &mut local_clock, &mut lamport_clock);
        assert_eq!(buffer.longest_row(), 2);
        buffer.edit(vec![10..12], "", &mut local_clock, &mut lamport_clock);
        assert_eq!(buffer.longest_row(), 0);
        buffer.edit(vec![24..24], "tuv", &mut local_clock, &mut lamport_clock);
        assert_eq!(buffer.longest_row(), 4);
    }

    #[test]
    fn test_iter_starting_at_point() {
        let mut buffer = Buffer::new("");
        let replica_id = Uuid::from_u128(1);
        let mut local_clock = time::Local::new(replica_id);
        let mut lamport_clock = time::Lamport::new(replica_id);
        buffer.edit(
            vec![0..0],
            "abcd\nefgh\nij",
            &mut local_clock,
            &mut lamport_clock,
        );
        buffer.edit(
            vec![12..12],
            "kl\nmno",
            &mut local_clock,
            &mut lamport_clock,
        );
        buffer.edit(vec![18..18], "\npqrs", &mut local_clock, &mut lamport_clock);
        buffer.edit(vec![18..21], "\nPQ", &mut local_clock, &mut lamport_clock);

        let cursor = buffer.iter_at_point(Point::new(0, 0));
        assert_eq!(cursor.into_string(), "abcd\nefgh\nijkl\nmno\nPQrs");

        let cursor = buffer.iter_at_point(Point::new(1, 0));
        assert_eq!(cursor.into_string(), "efgh\nijkl\nmno\nPQrs");

        let cursor = buffer.iter_at_point(Point::new(2, 0));
        assert_eq!(cursor.into_string(), "ijkl\nmno\nPQrs");

        let cursor = buffer.iter_at_point(Point::new(3, 0));
        assert_eq!(cursor.into_string(), "mno\nPQrs");

        let cursor = buffer.iter_at_point(Point::new(4, 0));
        assert_eq!(cursor.into_string(), "PQrs");

        let cursor = buffer.iter_at_point(Point::new(5, 0));
        assert_eq!(cursor.into_string(), "");

        let cursor = buffer.iter_at_point(Point::new(0, 0)).rev();
        assert_eq!(cursor.into_string(), "");

        let cursor = buffer.iter_at_point(Point::new(0, 3)).rev();
        assert_eq!(cursor.into_string(), "cba");

        let cursor = buffer.iter_at_point(Point::new(1, 4)).rev();
        assert_eq!(cursor.into_string(), "hgfe\ndcba");

        let cursor = buffer.iter_at_point(Point::new(3, 2)).rev();
        assert_eq!(cursor.into_string(), "nm\nlkji\nhgfe\ndcba");

        let cursor = buffer.iter_at_point(Point::new(4, 4)).rev();
        assert_eq!(cursor.into_string(), "srQP\nonm\nlkji\nhgfe\ndcba");

        let cursor = buffer.iter_at_point(Point::new(5, 0)).rev();
        assert_eq!(cursor.into_string(), "srQP\nonm\nlkji\nhgfe\ndcba");

        // Regression test:
        let mut buffer = Buffer::new("");
        let replica_id = Uuid::from_u128(1);
        let mut local_clock = time::Local::new(replica_id);
        let mut lamport_clock = time::Lamport::new(replica_id);
        buffer.edit(vec![0..0], "[workspace]\nmembers = [\n    \"xray_core\",\n    \"xray_server\",\n    \"xray_cli\",\n    \"xray_wasm\",\n]\n", &mut local_clock, &mut lamport_clock);
        buffer.edit(vec![60..60], "\n", &mut local_clock, &mut lamport_clock);

        let cursor = buffer.iter_at_point(Point::new(6, 0));
        assert_eq!(cursor.into_string(), "    \"xray_wasm\",\n]\n");
    }

    #[test]
    fn test_point_for_offset() {
        let text = Text::from("abc\ndefgh\nijklm\nopq");
        assert_eq!(text.point_for_offset(0), Ok(Point { row: 0, column: 0 }));
        assert_eq!(text.point_for_offset(1), Ok(Point { row: 0, column: 1 }));
        assert_eq!(text.point_for_offset(2), Ok(Point { row: 0, column: 2 }));
        assert_eq!(text.point_for_offset(3), Ok(Point { row: 0, column: 3 }));
        assert_eq!(text.point_for_offset(4), Ok(Point { row: 1, column: 0 }));
        assert_eq!(text.point_for_offset(5), Ok(Point { row: 1, column: 1 }));
        assert_eq!(text.point_for_offset(9), Ok(Point { row: 1, column: 5 }));
        assert_eq!(text.point_for_offset(10), Ok(Point { row: 2, column: 0 }));
        assert_eq!(text.point_for_offset(14), Ok(Point { row: 2, column: 4 }));
        assert_eq!(text.point_for_offset(15), Ok(Point { row: 2, column: 5 }));
        assert_eq!(text.point_for_offset(16), Ok(Point { row: 3, column: 0 }));
        assert_eq!(text.point_for_offset(17), Ok(Point { row: 3, column: 1 }));
        assert_eq!(text.point_for_offset(19), Ok(Point { row: 3, column: 3 }));
        assert_eq!(text.point_for_offset(20), Err(Error::OffsetOutOfRange));

        let text = Text::from("abc");
        assert_eq!(text.point_for_offset(0), Ok(Point { row: 0, column: 0 }));
        assert_eq!(text.point_for_offset(1), Ok(Point { row: 0, column: 1 }));
        assert_eq!(text.point_for_offset(2), Ok(Point { row: 0, column: 2 }));
        assert_eq!(text.point_for_offset(3), Ok(Point { row: 0, column: 3 }));
        assert_eq!(text.point_for_offset(4), Err(Error::OffsetOutOfRange));
    }

    #[test]
    fn test_offset_for_point() {
        let text = Text::from("abc\ndefgh");
        assert_eq!(text.offset_for_point(Point { row: 0, column: 0 }), Ok(0));
        assert_eq!(text.offset_for_point(Point { row: 0, column: 1 }), Ok(1));
        assert_eq!(text.offset_for_point(Point { row: 0, column: 2 }), Ok(2));
        assert_eq!(text.offset_for_point(Point { row: 0, column: 3 }), Ok(3));
        assert_eq!(
            text.offset_for_point(Point { row: 0, column: 4 }),
            Err(Error::OffsetOutOfRange)
        );
        assert_eq!(text.offset_for_point(Point { row: 1, column: 0 }), Ok(4));
        assert_eq!(text.offset_for_point(Point { row: 1, column: 1 }), Ok(5));
        assert_eq!(text.offset_for_point(Point { row: 1, column: 5 }), Ok(9));
        assert_eq!(
            text.offset_for_point(Point { row: 1, column: 6 }),
            Err(Error::OffsetOutOfRange)
        );

        let text = Text::from("abc");
        assert_eq!(text.offset_for_point(Point { row: 0, column: 0 }), Ok(0));
        assert_eq!(text.offset_for_point(Point { row: 0, column: 1 }), Ok(1));
        assert_eq!(text.offset_for_point(Point { row: 0, column: 2 }), Ok(2));
        assert_eq!(text.offset_for_point(Point { row: 0, column: 3 }), Ok(3));
        assert_eq!(
            text.offset_for_point(Point { row: 0, column: 4 }),
            Err(Error::OffsetOutOfRange)
        );
    }

    #[test]
    fn test_longest_row_in_range() {
        for seed in 0..100 {
            println!("{:?}", seed);
            let mut rng = StdRng::from_seed(&[seed]);
            let string = RandomCharIter(rng)
                .take(rng.gen_range(1, 10))
                .collect::<String>();
            let text = Text::from(string.as_ref());

            for _i in 0..10 {
                let end = rng.gen_range(1, string.len() + 1);
                let start = rng.gen_range(0, end);

                let mut cur_row = string[0..start].chars().filter(|c| *c == '\n').count() as u32;
                let mut cur_row_len = 0;
                let mut expected_longest_row = cur_row;
                let mut expected_longest_row_len = cur_row_len;
                for ch in string[start..end].chars() {
                    if ch == '\n' {
                        if cur_row_len > expected_longest_row_len {
                            expected_longest_row = cur_row;
                            expected_longest_row_len = cur_row_len;
                        }
                        cur_row += 1;
                        cur_row_len = 0;
                    } else {
                        cur_row_len += 1;
                    }
                }
                if cur_row_len > expected_longest_row_len {
                    expected_longest_row = cur_row;
                    expected_longest_row_len = cur_row_len;
                }

                assert_eq!(
                    text.longest_row_in_range(start..end),
                    Ok((expected_longest_row, expected_longest_row_len))
                );
            }
        }
    }

    #[test]
    fn test_fragment_ids() {
        for seed in 0..10 {
            use rand::{Rng, SeedableRng, StdRng};
            let mut rng = StdRng::from_seed(&[seed]);

            let mut ids = vec![FragmentId(Arc::new(vec![0])), FragmentId(Arc::new(vec![4]))];
            for _i in 0..100 {
                let index = rng.gen_range::<usize>(1, ids.len());

                let left = ids[index - 1].clone();
                let right = ids[index].clone();
                ids.insert(index, FragmentId::between_with_max(&left, &right, 4));

                let mut sorted_ids = ids.clone();
                sorted_ids.sort();
                assert_eq!(ids, sorted_ids);
            }
        }
    }

    #[test]
    fn test_anchors() {
        let mut buffer = Buffer::new("");
        let replica_id = Uuid::from_u128(1);
        let mut local_clock = time::Local::new(replica_id);
        let mut lamport_clock = time::Lamport::new(replica_id);
        buffer.edit(vec![0..0], "abc", &mut local_clock, &mut lamport_clock);
        let left_anchor = buffer.anchor_before_offset(2).unwrap();
        let right_anchor = buffer.anchor_after_offset(2).unwrap();

        buffer.edit(vec![1..1], "def\n", &mut local_clock, &mut lamport_clock);
        assert_eq!(buffer.to_string(), "adef\nbc");
        assert_eq!(buffer.offset_for_anchor(&left_anchor).unwrap(), 6);
        assert_eq!(buffer.offset_for_anchor(&right_anchor).unwrap(), 6);
        assert_eq!(
            buffer.point_for_anchor(&left_anchor).unwrap(),
            Point { row: 1, column: 1 }
        );
        assert_eq!(
            buffer.point_for_anchor(&right_anchor).unwrap(),
            Point { row: 1, column: 1 }
        );

        buffer.edit(vec![2..3], "", &mut local_clock, &mut lamport_clock);
        assert_eq!(buffer.to_string(), "adf\nbc");
        assert_eq!(buffer.offset_for_anchor(&left_anchor).unwrap(), 5);
        assert_eq!(buffer.offset_for_anchor(&right_anchor).unwrap(), 5);
        assert_eq!(
            buffer.point_for_anchor(&left_anchor).unwrap(),
            Point { row: 1, column: 1 }
        );
        assert_eq!(
            buffer.point_for_anchor(&right_anchor).unwrap(),
            Point { row: 1, column: 1 }
        );

        buffer.edit(vec![5..5], "ghi\n", &mut local_clock, &mut lamport_clock);
        assert_eq!(buffer.to_string(), "adf\nbghi\nc");
        assert_eq!(buffer.offset_for_anchor(&left_anchor).unwrap(), 5);
        assert_eq!(buffer.offset_for_anchor(&right_anchor).unwrap(), 9);
        assert_eq!(
            buffer.point_for_anchor(&left_anchor).unwrap(),
            Point { row: 1, column: 1 }
        );
        assert_eq!(
            buffer.point_for_anchor(&right_anchor).unwrap(),
            Point { row: 2, column: 0 }
        );

        buffer.edit(vec![7..9], "", &mut local_clock, &mut lamport_clock);
        assert_eq!(buffer.to_string(), "adf\nbghc");
        assert_eq!(buffer.offset_for_anchor(&left_anchor).unwrap(), 5);
        assert_eq!(buffer.offset_for_anchor(&right_anchor).unwrap(), 7);
        assert_eq!(
            buffer.point_for_anchor(&left_anchor).unwrap(),
            Point { row: 1, column: 1 }
        );
        assert_eq!(
            buffer.point_for_anchor(&right_anchor).unwrap(),
            Point { row: 1, column: 3 }
        );

        // Ensure anchoring to a point is equivalent to anchoring to an offset.
        assert_eq!(
            buffer.anchor_before_point(Point { row: 0, column: 0 }),
            buffer.anchor_before_offset(0)
        );
        assert_eq!(
            buffer.anchor_before_point(Point { row: 0, column: 1 }),
            buffer.anchor_before_offset(1)
        );
        assert_eq!(
            buffer.anchor_before_point(Point { row: 0, column: 2 }),
            buffer.anchor_before_offset(2)
        );
        assert_eq!(
            buffer.anchor_before_point(Point { row: 0, column: 3 }),
            buffer.anchor_before_offset(3)
        );
        assert_eq!(
            buffer.anchor_before_point(Point { row: 1, column: 0 }),
            buffer.anchor_before_offset(4)
        );
        assert_eq!(
            buffer.anchor_before_point(Point { row: 1, column: 1 }),
            buffer.anchor_before_offset(5)
        );
        assert_eq!(
            buffer.anchor_before_point(Point { row: 1, column: 2 }),
            buffer.anchor_before_offset(6)
        );
        assert_eq!(
            buffer.anchor_before_point(Point { row: 1, column: 3 }),
            buffer.anchor_before_offset(7)
        );
        assert_eq!(
            buffer.anchor_before_point(Point { row: 1, column: 4 }),
            buffer.anchor_before_offset(8)
        );

        // Comparison between anchors.
        let anchor_at_offset_0 = buffer.anchor_before_offset(0).unwrap();
        let anchor_at_offset_1 = buffer.anchor_before_offset(1).unwrap();
        let anchor_at_offset_2 = buffer.anchor_before_offset(2).unwrap();

        assert_eq!(
            buffer.cmp_anchors(&anchor_at_offset_0, &anchor_at_offset_0),
            Ok(Ordering::Equal)
        );
        assert_eq!(
            buffer.cmp_anchors(&anchor_at_offset_1, &anchor_at_offset_1),
            Ok(Ordering::Equal)
        );
        assert_eq!(
            buffer.cmp_anchors(&anchor_at_offset_2, &anchor_at_offset_2),
            Ok(Ordering::Equal)
        );

        assert_eq!(
            buffer.cmp_anchors(&anchor_at_offset_0, &anchor_at_offset_1),
            Ok(Ordering::Less)
        );
        assert_eq!(
            buffer.cmp_anchors(&anchor_at_offset_1, &anchor_at_offset_2),
            Ok(Ordering::Less)
        );
        assert_eq!(
            buffer.cmp_anchors(&anchor_at_offset_0, &anchor_at_offset_2),
            Ok(Ordering::Less)
        );

        assert_eq!(
            buffer.cmp_anchors(&anchor_at_offset_1, &anchor_at_offset_0),
            Ok(Ordering::Greater)
        );
        assert_eq!(
            buffer.cmp_anchors(&anchor_at_offset_2, &anchor_at_offset_1),
            Ok(Ordering::Greater)
        );
        assert_eq!(
            buffer.cmp_anchors(&anchor_at_offset_2, &anchor_at_offset_0),
            Ok(Ordering::Greater)
        );
    }

    #[test]
    fn test_anchors_at_start_and_end() {
        let mut buffer = Buffer::new("");
        let replica_id = Uuid::from_u128(1);
        let mut local_clock = time::Local::new(replica_id);
        let mut lamport_clock = time::Lamport::new(replica_id);
        let before_start_anchor = buffer.anchor_before_offset(0).unwrap();
        let after_end_anchor = buffer.anchor_after_offset(0).unwrap();

        buffer.edit(vec![0..0], "abc", &mut local_clock, &mut lamport_clock);
        assert_eq!(buffer.to_string(), "abc");
        assert_eq!(buffer.offset_for_anchor(&before_start_anchor).unwrap(), 0);
        assert_eq!(buffer.offset_for_anchor(&after_end_anchor).unwrap(), 3);

        let after_start_anchor = buffer.anchor_after_offset(0).unwrap();
        let before_end_anchor = buffer.anchor_before_offset(3).unwrap();

        buffer.edit(vec![3..3], "def", &mut local_clock, &mut lamport_clock);
        buffer.edit(vec![0..0], "ghi", &mut local_clock, &mut lamport_clock);
        assert_eq!(buffer.to_string(), "ghiabcdef");
        assert_eq!(buffer.offset_for_anchor(&before_start_anchor).unwrap(), 0);
        assert_eq!(buffer.offset_for_anchor(&after_start_anchor).unwrap(), 3);
        assert_eq!(buffer.offset_for_anchor(&before_end_anchor).unwrap(), 6);
        assert_eq!(buffer.offset_for_anchor(&after_end_anchor).unwrap(), 9);
    }

    #[test]
    fn test_is_modified() {
        let mut buffer = Buffer::new("abc");
        let replica_id = Uuid::from_u128(1);
        let mut local_clock = time::Local::new(replica_id);
        let mut lamport_clock = time::Lamport::new(replica_id);

        assert!(!buffer.is_modified());
        buffer.edit(vec![1..2], "", &mut local_clock, &mut lamport_clock);
        assert!(buffer.is_modified());
    }

    #[test]
    fn test_random_concurrent_edits() {
        use crate::tests::Network;

        const PEERS: usize = 3;

        for seed in 0..50 {
            println!("{:?}", seed);
            let mut rng = StdRng::from_seed(&[seed]);

            let base_text = RandomCharIter(rng)
                .take(rng.gen_range(0, 10))
                .collect::<String>();
            let mut replica_ids = Vec::new();
            let mut buffers = Vec::new();
            let mut local_clocks = Vec::new();
            let mut lamport_clocks = Vec::new();
            let mut network = Network::new();
            for i in 0..PEERS {
                let buffer = Buffer::new(base_text.as_str());
                buffers.push(buffer);
                let replica_id = Uuid::from_u128((i + 1) as u128);
                replica_ids.push(replica_id);
                local_clocks.push(time::Local::new(replica_id));
                lamport_clocks.push(time::Lamport::new(replica_id));
                network.add_peer(replica_id);
            }

            let mut mutation_count = 10;
            loop {
                let replica_index = rng.gen_range(0, PEERS);
                let replica_id = replica_ids[replica_index];
                let buffer = &mut buffers[replica_index];
                let local_clock = &mut local_clocks[replica_index];
                let lamport_clock = &mut lamport_clocks[replica_index];
                if mutation_count > 0 && rng.gen() {
                    let (_, _, ops) = buffer.randomly_mutate(&mut rng, local_clock, lamport_clock);
                    network.broadcast(replica_id, ops, &mut rng);
                    mutation_count -= 1;
                } else if network.has_unreceived(replica_id) {
                    buffer
                        .apply_ops(
                            network.receive(replica_id, &mut rng),
                            local_clock,
                            lamport_clock,
                        )
                        .unwrap();
                }

                if mutation_count == 0 && network.is_idle() {
                    break;
                }
            }

            for buffer in &buffers[1..] {
                assert_eq!(buffer.to_string(), buffers[0].to_string());
                assert_eq!(
                    buffer.selections().collect::<HashMap<_, _>>(),
                    buffers[0].selections().collect::<HashMap<_, _>>()
                );
            }
        }
    }

    struct RandomCharIter<T: Rng>(T);

    impl<T: Rng> Iterator for RandomCharIter<T> {
        type Item = char;

        fn next(&mut self) -> Option<Self::Item> {
            if self.0.gen_weighted_bool(5) {
                Some('\n')
            } else {
                Some(self.0.gen_range(b'a', b'z' + 1).into())
            }
        }
    }

    impl Buffer {
        pub fn randomly_mutate<T>(
            &mut self,
            rng: &mut T,
            local_clock: &mut time::Local,
            lamport_clock: &mut time::Lamport,
        ) -> (Vec<Range<usize>>, String, Vec<Operation>)
        where
            T: Rng,
        {
            // Randomly mutate text.
            let mut old_ranges: Vec<Range<usize>> = Vec::new();
            for _ in 0..5 {
                let last_end = old_ranges.last().map_or(0, |last_range| last_range.end + 1);
                if last_end > self.len() {
                    break;
                }
                let end = rng.gen_range::<usize>(last_end, self.len() + 1);
                let start = rng.gen_range::<usize>(last_end, end + 1);
                old_ranges.push(start..end);
            }
            let new_text_len = rng.gen_range(0, 10);
            let new_text: String = RandomCharIter(&mut *rng).take(new_text_len).collect();

            if rng.gen_weighted_bool(5) {
                local_clock.tick();
            }

            let mut operations = self.edit(
                old_ranges.iter().cloned(),
                new_text.as_str(),
                local_clock,
                lamport_clock,
            );

            // Randomly add, remove or mutate selection sets.
            let replica_selection_sets = &self
                .selections()
                .map(|(set_id, _)| *set_id)
                .filter(|set_id| local_clock.replica_id == set_id.replica_id)
                .collect::<Vec<_>>();
            let set_id = rng.choose(&replica_selection_sets);
            if set_id.is_some() && rng.gen() {
                let op = self
                    .remove_selection_set(*set_id.unwrap(), local_clock, lamport_clock)
                    .unwrap();
                operations.push(op);
            } else {
                let mut selections = Vec::new();
                for _ in 0..rng.gen_range(1, 5) {
                    let start = rng.gen_range(0, self.len() + 1);
                    let end = rng.gen_range(0, self.len() + 1);
                    let selection = if start > end {
                        Selection {
                            start: self.anchor_before_offset(end).unwrap(),
                            end: self.anchor_before_offset(start).unwrap(),
                            reversed: true,
                        }
                    } else {
                        Selection {
                            start: self.anchor_before_offset(start).unwrap(),
                            end: self.anchor_before_offset(end).unwrap(),
                            reversed: false,
                        }
                    };
                    selections.push(selection);
                }

                let op = if let Some(set_id) = set_id {
                    self.mutate_selections(*set_id, local_clock, lamport_clock, |_, set| {
                        *set = selections
                    })
                    .unwrap()
                } else {
                    self.add_selection_set(selections, local_clock, lamport_clock)
                        .1
                };
                operations.push(op);
            }

            (old_ranges, new_text, operations)
        }
    }
}
