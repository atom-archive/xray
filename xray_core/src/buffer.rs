use std::cell::RefCell;
use std::cmp;
use std::collections::{HashMap, HashSet};
use std::iter;
use std::ops::{Add, AddAssign, Range, Sub};
use std::result;
use std::sync::Arc;
use super::tree::{self, SeekBias, Tree};
use notify_cell::NotifyCell;

pub type ReplicaId = usize;
type LocalTimestamp = usize;
type LamportTimestamp = usize;
type Result<T> = result::Result<T, Error>;

#[derive(Eq, PartialEq, Debug)]
pub enum Error {
    OffsetOutOfRange,
    InvalidAnchor,
}

#[derive(Debug)]
pub struct Buffer {
    replica_id: ReplicaId,
    local_clock: LocalTimestamp,
    lamport_clock: LamportTimestamp,
    fragments: Tree<Fragment>,
    insertions: HashMap<ChangeId, Tree<FragmentMapping>>,
    anchor_cache: RefCell<HashMap<Anchor, (usize, Point)>>,
    offset_cache: RefCell<HashMap<Point, usize>>,
    pub version: NotifyCell<Version>,
}

#[derive(Clone, Copy, Debug)]
pub struct Version(LocalTimestamp);

#[derive(Clone, Copy, Eq, PartialEq, Debug, Serialize, Hash)]
pub struct Point {
    pub row: u32,
    pub column: u32,
}

#[derive(Clone, Eq, PartialEq, Debug, Hash)]
pub struct Anchor(AnchorInner);

#[derive(Clone, Eq, PartialEq, Debug, Hash)]
enum AnchorInner {
    Start,
    End,
    Middle {
        insertion_id: ChangeId,
        offset: usize,
        bias: AnchorBias,
    },
}

#[derive(Clone, Eq, PartialEq, Debug, Hash)]
enum AnchorBias {
    Left,
    Right,
}

pub struct Iter<'a> {
    fragment_cursor: tree::Cursor<'a, Fragment>,
    fragment_offset: usize,
}

#[derive(Eq, PartialEq, Debug)]
struct Insertion {
    id: ChangeId,
    parent_id: ChangeId,
    offset_in_parent: usize,
    replica_id: ReplicaId,
    lamport_timestamp: LamportTimestamp,
    text: Text,
}

#[derive(Eq, PartialEq, Debug)]
pub struct Text {
    code_units: Vec<u16>,
    newline_offsets: Vec<usize>,
}

#[derive(Hash, Eq, PartialEq, Clone, Copy, Debug, Serialize)]
struct ChangeId {
    replica_id: ReplicaId,
    local_timestamp: LocalTimestamp,
}

#[derive(Ord, PartialOrd, Eq, PartialEq, Clone, Debug)]
struct FragmentId(Arc<Vec<u16>>);

#[derive(Eq, PartialEq, Clone, Debug)]
struct Fragment {
    id: FragmentId,
    insertion: Arc<Insertion>,
    start_offset: usize,
    end_offset: usize,
    deletions: HashSet<ChangeId>,
}

#[derive(Eq, PartialEq, Clone, Debug)]
pub struct FragmentSummary {
    extent: usize,
    extent_2d: Point,
    max_fragment_id: FragmentId,
}

#[derive(Ord, PartialOrd, Eq, PartialEq, Clone, Copy, Debug)]
struct CharacterCount(usize);

#[derive(Ord, PartialOrd, Eq, PartialEq, Clone, Copy, Debug)]
struct NewlineCount(usize);

#[derive(Eq, PartialEq, Clone, Debug)]
struct FragmentMapping {
    extent: usize,
    fragment_id: FragmentId,
}

#[derive(Eq, PartialEq, Clone, Debug)]
struct FragmentMappingSummary {
    extent: usize,
}

#[derive(Ord, PartialOrd, Eq, PartialEq, Clone, Copy, Debug)]
struct InsertionOffset(usize);

impl Buffer {
    pub fn new(replica_id: ReplicaId) -> Self {
        assert!(replica_id > 0);
        let mut fragments = Tree::<Fragment>::new();

        // Push start sentinel.
        fragments.push(Fragment::new(
            FragmentId::min_value(),
            Insertion {
                id: ChangeId {
                    replica_id: 0,
                    local_timestamp: 0,
                },
                parent_id: ChangeId {
                    replica_id: 0,
                    local_timestamp: 0,
                },
                offset_in_parent: 0,
                replica_id: 0,
                lamport_timestamp: 0,
                text: Text::new(vec![]),
            },
        ));

        Buffer {
            replica_id,
            local_clock: 0,
            lamport_clock: 0,
            fragments,
            insertions: HashMap::new(),
            anchor_cache: RefCell::new(HashMap::new()),
            offset_cache: RefCell::new(HashMap::new()),
            version: NotifyCell::new(Version(0)),
        }
    }

    pub fn len(&self) -> usize {
        self.fragments.len::<CharacterCount>().0
    }

    pub fn len_for_row(&self, row: u32) -> Result<u32> {
        let row_start_offset = self.offset_for_point(Point::new(row, 0))?;
        let row_end_offset = if row >= self.max_point().row {
            self.len()
        } else {
            self.offset_for_point(Point::new(row + 1, 0))? - 1
        };

        Ok((row_end_offset - row_start_offset) as u32)
    }

    pub fn max_point(&self) -> Point {
        self.fragments.len::<Point>()
    }

    pub fn to_u16_chars(&self) -> Vec<u16> {
        let mut result = Vec::with_capacity(self.len());
        result.extend(self.iter());
        result
    }

    #[cfg(test)]
    pub fn to_string(&self) -> String {
        String::from_utf16_lossy(self.iter().collect::<Vec<u16>>().as_slice())
    }

    pub fn iter(&self) -> Iter {
        Iter::new(self)
    }

    pub fn iter_starting_at_row(&self, row: u32) -> Iter {
        Iter::starting_at_row(self, row)
    }

    pub fn splice<T: Into<Text>>(&mut self, old_range: Range<usize>, new_text: T) {
        let new_text = new_text.into();
        let new_text = if new_text.len() > 0 {
            Some(new_text)
        } else {
            None
        };
        if new_text.is_some() || old_range.end > old_range.start {
            self.local_clock += 1;
            self.lamport_clock += 1;
            let change_id = ChangeId {
                replica_id: self.replica_id,
                local_timestamp: self.local_clock,
            };
            self.splice_fragments(change_id, old_range, new_text);
            self.anchor_cache.borrow_mut().clear();
            self.offset_cache.borrow_mut().clear();
            self.version.set(Version(self.local_clock));
        }
    }

    fn splice_fragments(
        &mut self,
        change_id: ChangeId,
        old_range: Range<usize>,
        mut new_text: Option<Text>,
    ) {
        let old_fragments = self.fragments.clone();
        let mut cursor = old_fragments.cursor();
        let mut updated_fragments =
            cursor.build_prefix(&CharacterCount(old_range.start), SeekBias::Right);
        let mut inserted_fragments = Vec::new();

        if cursor.prev_item().is_none() {
            inserted_fragments.push(cursor.item().unwrap().clone());
            cursor.next();
        }

        let prev_fragment = cursor.prev_item().unwrap();
        if let Some(cur_fragment) = cursor.item() {
            let (before_range, within_range, after_range) = self.split_fragment(
                prev_fragment,
                cur_fragment,
                cursor.start::<CharacterCount>().0,
                &old_range,
            );
            let insertion = new_text.take().map(|new_text| {
                self.build_insertion(
                    change_id.clone(),
                    before_range.as_ref().unwrap_or(prev_fragment),
                    within_range.as_ref().or(after_range.as_ref()),
                    new_text,
                )
            });

            before_range.map(|fragment| inserted_fragments.push(fragment));
            insertion.map(|fragment| inserted_fragments.push(fragment));
            within_range.map(|mut fragment| {
                fragment.deletions.insert(change_id.clone());
                inserted_fragments.push(fragment);
            });
            after_range.map(|fragment| inserted_fragments.push(fragment));
            cursor.next();
        } else {
            new_text.take().map(|new_text| {
                inserted_fragments.push(self.build_insertion(
                    change_id,
                    prev_fragment,
                    None,
                    new_text,
                ));
            });
        }

        loop {
            let fragment_start = cursor.start::<CharacterCount>().0;

            if fragment_start >= old_range.end {
                break;
            }

            let prev_fragment = cursor.prev_item().unwrap();
            let cur_fragment = cursor.item().unwrap();
            let fragment_end = fragment_start + cur_fragment.len();

            if old_range.end < fragment_end {
                let (_, within_range, after_range) =
                    self.split_fragment(prev_fragment, cur_fragment, fragment_start, &old_range);
                let mut within_range = within_range.unwrap();
                within_range.deletions.insert(change_id.clone());
                inserted_fragments.push(within_range);
                inserted_fragments.push(after_range.unwrap());
            } else {
                let mut fragment = cur_fragment.clone();
                if fragment.is_visible() {
                    fragment.deletions.insert(change_id.clone());
                }
                inserted_fragments.push(fragment)
            }

            cursor.next();
        }

        updated_fragments.extend(inserted_fragments);
        updated_fragments.push_tree(cursor.build_suffix());
        self.fragments = updated_fragments;
    }

    fn split_fragment(
        &mut self,
        prev_fragment: &Fragment,
        fragment: &Fragment,
        fragment_start: usize,
        range: &Range<usize>,
    ) -> (Option<Fragment>, Option<Fragment>, Option<Fragment>) {
        let fragment_end = fragment_start + fragment.len();
        let mut prefix = fragment.clone();
        let mut before_range = None;
        let mut within_range = None;
        let mut after_range = None;

        if range.end < fragment_end {
            let mut suffix = prefix.clone();
            suffix.start_offset = prefix.start_offset + range.end - fragment_start;
            prefix.end_offset = suffix.start_offset;
            prefix.id = FragmentId::between(&prev_fragment.id, &suffix.id);
            after_range = Some(suffix);
        }

        if range.start < range.end {
            let mut suffix = prefix.clone();
            suffix.start_offset =
                prefix.start_offset + cmp::max(range.start, fragment_start) - fragment_start;
            prefix.end_offset = suffix.start_offset;
            prefix.id = FragmentId::between(&prev_fragment.id, &suffix.id);
            within_range = Some(suffix);
        }

        if range.start > fragment_start {
            before_range = Some(prefix);
        }

        if within_range.is_some() || after_range.is_some() {
            let mut updated_split_tree;
            {
                let split_tree = self.insertions.get(&fragment.insertion.id).unwrap();
                let mut cursor = split_tree.cursor();
                updated_split_tree =
                    cursor.build_prefix(&InsertionOffset(fragment.start_offset), SeekBias::Right);

                if let Some(ref fragment) = before_range {
                    updated_split_tree.push(FragmentMapping {
                        extent: range.start - fragment_start,
                        fragment_id: fragment.id.clone(),
                    })
                }

                if let Some(ref fragment) = within_range {
                    updated_split_tree.push(FragmentMapping {
                        extent: range.end - range.start,
                        fragment_id: fragment.id.clone(),
                    })
                }
                if let Some(ref fragment) = after_range {
                    updated_split_tree.push(FragmentMapping {
                        extent: fragment_end - range.end,
                        fragment_id: fragment.id.clone(),
                    })
                }

                cursor.next();
                updated_split_tree.push_tree(cursor.build_suffix());
            }

            self.insertions
                .insert(fragment.insertion.id, updated_split_tree);
        }

        (before_range, within_range, after_range)
    }

    fn build_insertion(
        &mut self,
        change_id: ChangeId,
        prev_fragment: &Fragment,
        next_fragment: Option<&Fragment>,
        text: Text,
    ) -> Fragment {
        let new_fragment_id = FragmentId::between(
            &prev_fragment.id,
            next_fragment
                .map(|f| &f.id)
                .unwrap_or(&FragmentId::max_value()),
        );

        let mut split_tree = Tree::new();
        split_tree.push(FragmentMapping {
            extent: text.len(),
            fragment_id: new_fragment_id.clone(),
        });
        self.insertions.insert(change_id, split_tree);

        Fragment::new(
            new_fragment_id,
            Insertion {
                id: change_id,
                parent_id: prev_fragment.insertion.id,
                offset_in_parent: prev_fragment.end_offset,
                replica_id: self.replica_id,
                lamport_timestamp: self.lamport_clock,
                text,
            },
        )
    }

    pub fn anchor_before_offset(&self, offset: usize) -> Result<Anchor> {
        self.anchor_for_offset(offset, AnchorBias::Left)
    }

    pub fn anchor_after_offset(&self, offset: usize) -> Result<Anchor> {
        self.anchor_for_offset(offset, AnchorBias::Right)
    }

    fn anchor_for_offset(&self, offset: usize, bias: AnchorBias) -> Result<Anchor> {
        let max_offset = self.len();
        if offset > max_offset {
            return Err(Error::OffsetOutOfRange);
        }

        let seek_bias;
        match bias {
            AnchorBias::Left => {
                if offset == 0 {
                    return Ok(Anchor(AnchorInner::Start));
                } else {
                    seek_bias = SeekBias::Left;
                }
            }
            AnchorBias::Right => {
                if offset == max_offset {
                    return Ok(Anchor(AnchorInner::End));
                } else {
                    seek_bias = SeekBias::Right;
                }
            }
        };

        let mut cursor = self.fragments.cursor();
        cursor.seek(&CharacterCount(offset), seek_bias);
        let fragment = cursor.item().unwrap();
        let offset_in_fragment = offset - cursor.start::<CharacterCount>().0;
        let offset_in_insertion = fragment.start_offset + offset_in_fragment;
        let point = cursor.start::<Point>() + &fragment.point_for_offset(offset_in_fragment)?;
        let anchor = Anchor(AnchorInner::Middle {
            insertion_id: fragment.insertion.id,
            offset: offset_in_insertion,
            bias,
        });
        self.cache_position(Some(anchor.clone()), offset, point);
        Ok(anchor)
    }

    pub fn anchor_before_point(&self, point: Point) -> Result<Anchor> {
        self.anchor_for_point(point, AnchorBias::Left)
    }

    pub fn anchor_after_point(&self, point: Point) -> Result<Anchor> {
        self.anchor_for_point(point, AnchorBias::Right)
    }

    fn anchor_for_point(&self, point: Point, bias: AnchorBias) -> Result<Anchor> {
        let max_point = self.max_point();
        if point > max_point {
            return Err(Error::OffsetOutOfRange);
        }

        let seek_bias;
        match bias {
            AnchorBias::Left => {
                if point.is_zero() {
                    return Ok(Anchor(AnchorInner::Start));
                } else {
                    seek_bias = SeekBias::Left;
                }
            }
            AnchorBias::Right => {
                if point == max_point {
                    return Ok(Anchor(AnchorInner::End));
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
        let anchor = Anchor(AnchorInner::Middle {
            insertion_id: fragment.insertion.id,
            offset: offset_in_insertion,
            bias,
        });
        let offset = cursor.start::<CharacterCount>().0 + offset_in_fragment;
        self.cache_position(Some(anchor.clone()), offset, point);
        Ok(anchor)
    }

    pub fn offset_for_anchor(&self, anchor: &Anchor) -> Result<usize> {
        Ok(self.position_for_anchor(anchor)?.0)
    }

    pub fn point_for_anchor(&self, anchor: &Anchor) -> Result<Point> {
        Ok(self.position_for_anchor(anchor)?.1)
    }

    fn position_for_anchor(&self, anchor: &Anchor) -> Result<(usize, Point)> {
        match &anchor.0 {
            &AnchorInner::Start => Ok((0, Point { row: 0, column: 0 })),
            &AnchorInner::End => Ok((self.len(), self.fragments.len::<Point>())),
            &AnchorInner::Middle {
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
                        &AnchorBias::Left => SeekBias::Left,
                        &AnchorBias::Right => SeekBias::Right,
                    };

                    let splits = self.insertions
                        .get(&insertion_id)
                        .ok_or(Error::InvalidAnchor)?;
                    let mut splits_cursor = splits.cursor();
                    splits_cursor.seek(&InsertionOffset(offset), seek_bias);
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
                                    let offset =
                                        fragments_cursor.start::<CharacterCount>().0 + overshoot;
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

    fn offset_for_point(&self, point: Point) -> Result<usize> {
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
                    let offset = &fragments_cursor.start::<CharacterCount>().0 + &overshoot;
                    self.cache_position(None, offset, point);
                    offset
                })
        }
    }

    pub fn cmp_anchors(&self, a: &Anchor, b: &Anchor) -> Result<cmp::Ordering> {
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

    pub fn is_zero(&self) -> bool {
        self.row == 0 && self.column == 0
    }
}

impl tree::Dimension for Point {
    type Summary = FragmentSummary;

    fn from_summary(summary: &Self::Summary) -> Self {
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

impl AddAssign for Point {
    fn add_assign(&mut self, other: Self) {
        if other.row == 0 {
            self.column += other.column;
        } else {
            self.row += other.row;
            self.column = other.column;
        }
    }
}

impl PartialOrd for Point {
    fn partial_cmp(&self, other: &Point) -> Option<cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Point {
    fn cmp(&self, other: &Point) -> cmp::Ordering {
        let a = (self.row as usize) << 32 | self.column as usize;
        let b = (other.row as usize) << 32 | other.column as usize;
        a.cmp(&b)
    }
}
impl<'a> Iter<'a> {
    fn new(buffer: &'a Buffer) -> Self {
        let mut fragment_cursor = buffer.fragments.cursor();
        fragment_cursor.seek(&CharacterCount(0), SeekBias::Right);
        Self {
            fragment_cursor,
            fragment_offset: 0,
        }
    }

    fn starting_at_row(buffer: &'a Buffer, target_row: u32) -> Self {
        let mut fragment_cursor = buffer.fragments.cursor();
        fragment_cursor.seek(
            &Point {
                row: target_row,
                column: 0,
            },
            SeekBias::Right,
        );

        let mut fragment_offset = 0;
        if let Some(fragment) = fragment_cursor.item() {
            let fragment_start_row = fragment_cursor.start::<Point>().row;
            if target_row != fragment_start_row {
                let target_row_within_fragment = target_row - fragment_start_row - 1;
                fragment_offset = fragment.insertion.text.newline_offsets
                    [target_row_within_fragment as usize] + 1;
            }
        }

        Self {
            fragment_cursor,
            fragment_offset,
        }
    }
}

impl<'a> Iterator for Iter<'a> {
    type Item = u16;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(fragment) = self.fragment_cursor.item() {
            if let Some(c) = fragment.get_code_unit(self.fragment_offset) {
                self.fragment_offset += 1;
                return Some(c);
            }
        }

        loop {
            self.fragment_cursor.next();
            if let Some(fragment) = self.fragment_cursor.item() {
                if let Some(c) = fragment.get_code_unit(0) {
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

impl Text {
    fn new(code_units: Vec<u16>) -> Self {
        let newline_offsets = code_units
            .iter()
            .enumerate()
            .filter_map(|(offset, c)| {
                if *c == (b'\n' as u16) {
                    Some(offset)
                } else {
                    None
                }
            })
            .collect();

        Self {
            code_units,
            newline_offsets,
        }
    }

    fn len(&self) -> usize {
        self.code_units.len()
    }

    fn point_for_offset(&self, offset: usize) -> Result<Point> {
        if offset > self.len() {
            Err(Error::OffsetOutOfRange)
        } else {
            let row = find_insertion_index(&self.newline_offsets, &offset);
            let row_start_offset = if row == 0 {
                0
            } else {
                self.newline_offsets[row - 1] + 1
            };

            Ok(Point {
                row: row as u32,
                column: (offset - row_start_offset) as u32,
            })
        }
    }

    fn offset_for_point(&self, point: Point) -> Result<usize> {
        let row_start_offset = if point.row == 0 {
            0
        } else {
            self.newline_offsets[(point.row - 1) as usize] + 1
        };

        let row_end_offset = if self.newline_offsets.len() > point.row as usize {
            self.newline_offsets[point.row as usize]
        } else {
            self.len()
        };

        let target_offset = row_start_offset + point.column as usize;
        if target_offset <= row_end_offset {
            Ok(target_offset)
        } else {
            Err(Error::OffsetOutOfRange)
        }
    }
}

impl<'a> From<&'a str> for Text {
    fn from(s: &'a str) -> Self {
        Self::new(s.encode_utf16().collect())
    }
}

impl<'a> From<Vec<u16>> for Text {
    fn from(s: Vec<u16>) -> Self {
        Self::new(s)
    }
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

impl tree::Dimension for FragmentId {
    type Summary = FragmentSummary;

    fn from_summary(summary: &Self::Summary) -> Self {
        summary.max_fragment_id.clone()
    }
}

impl<'a> Add<&'a Self> for FragmentId {
    type Output = FragmentId;

    fn add(self, other: &'a Self) -> Self::Output {
        cmp::max(&self, other).clone()
    }
}

impl AddAssign for FragmentId {
    fn add_assign(&mut self, other: Self) {
        if *self < other {
            *self = other
        }
    }
}

impl Fragment {
    fn new(id: FragmentId, ins: Insertion) -> Self {
        let end_offset = ins.text.len();
        Self {
            id,
            insertion: Arc::new(ins),
            start_offset: 0,
            end_offset,
            deletions: HashSet::new(),
        }
    }

    fn get_code_unit(&self, offset: usize) -> Option<u16> {
        if offset < self.len() {
            Some(self.insertion.text.code_units[self.start_offset + offset].clone())
        } else {
            None
        }
    }

    fn len(&self) -> usize {
        if self.is_visible() {
            self.end_offset - self.start_offset
        } else {
            0
        }
    }

    fn is_visible(&self) -> bool {
        self.deletions.is_empty()
    }

    fn point_for_offset(&self, offset: usize) -> Result<Point> {
        let text = &self.insertion.text;
        let offset_in_insertion = self.start_offset + offset;
        Ok(
            text.point_for_offset(offset_in_insertion)?
                - &text.point_for_offset(self.start_offset)?,
        )
    }

    fn offset_for_point(&self, point: Point) -> Result<usize> {
        let text = &self.insertion.text;
        let point_in_insertion = text.point_for_offset(self.start_offset)? + &point;
        Ok(text.offset_for_point(point_in_insertion)? - self.start_offset)
    }
}

impl tree::Item for Fragment {
    type Summary = FragmentSummary;

    fn summarize(&self) -> Self::Summary {
        if self.is_visible() {
            let fragment_2d_start = self.insertion
                .text
                .point_for_offset(self.start_offset)
                .unwrap();
            let fragment_2d_end = self.insertion
                .text
                .point_for_offset(self.end_offset)
                .unwrap();
            FragmentSummary {
                extent: self.len(),
                extent_2d: fragment_2d_end - &fragment_2d_start,
                max_fragment_id: self.id.clone(),
            }
        } else {
            FragmentSummary {
                extent: 0,
                extent_2d: Point { row: 0, column: 0 },
                max_fragment_id: self.id.clone(),
            }
        }
    }
}

impl<'a> AddAssign<&'a FragmentSummary> for FragmentSummary {
    fn add_assign(&mut self, other: &Self) {
        self.extent += other.extent;
        self.extent_2d += other.extent_2d;
        if self.max_fragment_id < other.max_fragment_id {
            self.max_fragment_id = other.max_fragment_id.clone();
        }
    }
}

impl Default for FragmentSummary {
    fn default() -> Self {
        FragmentSummary {
            extent: 0,
            extent_2d: Point { row: 0, column: 0 },
            max_fragment_id: FragmentId::min_value(),
        }
    }
}

impl tree::Dimension for CharacterCount {
    type Summary = FragmentSummary;

    fn from_summary(summary: &Self::Summary) -> Self {
        CharacterCount(summary.extent)
    }
}

impl<'a> Add<&'a Self> for CharacterCount {
    type Output = CharacterCount;

    fn add(self, other: &'a Self) -> Self::Output {
        CharacterCount(self.0 + other.0)
    }
}

impl AddAssign for CharacterCount {
    fn add_assign(&mut self, other: Self) {
        self.0 += other.0;
    }
}

impl tree::Item for FragmentMapping {
    type Summary = FragmentMappingSummary;

    fn summarize(&self) -> Self::Summary {
        FragmentMappingSummary {
            extent: self.extent,
        }
    }
}

impl<'a> AddAssign<&'a FragmentMappingSummary> for FragmentMappingSummary {
    fn add_assign(&mut self, other: &Self) {
        self.extent += other.extent;
    }
}

impl Default for FragmentMappingSummary {
    fn default() -> Self {
        FragmentMappingSummary { extent: 0 }
    }
}

impl tree::Dimension for InsertionOffset {
    type Summary = FragmentMappingSummary;

    fn from_summary(summary: &Self::Summary) -> Self {
        InsertionOffset(summary.extent)
    }
}

impl<'a> Add<&'a Self> for InsertionOffset {
    type Output = InsertionOffset;

    fn add(self, other: &'a Self) -> Self::Output {
        InsertionOffset(self.0 + other.0)
    }
}

impl AddAssign for InsertionOffset {
    fn add_assign(&mut self, other: Self) {
        self.0 += other.0;
    }
}

fn find_insertion_index<T: Ord>(v: &Vec<T>, x: &T) -> usize {
    match v.binary_search(x) {
        Ok(index) => index,
        Err(index) => index,
    }
}

#[cfg(test)]
mod tests {
    extern crate rand;

    use super::*;
    use std::cmp::Ordering;

    #[test]
    fn splice() {
        let mut buffer = Buffer::new(1);
        buffer.splice(0..0, "abc");
        assert_eq!(buffer.to_string(), "abc");
        buffer.splice(3..3, "def");
        assert_eq!(buffer.to_string(), "abcdef");
        buffer.splice(0..0, "ghi");
        assert_eq!(buffer.to_string(), "ghiabcdef");
        buffer.splice(5..5, "jkl");
        assert_eq!(buffer.to_string(), "ghiabjklcdef");
        buffer.splice(6..7, "");
        assert_eq!(buffer.to_string(), "ghiabjlcdef");
        buffer.splice(4..9, "mno");
        assert_eq!(buffer.to_string(), "ghiamnoef");
    }

    #[test]
    fn random_splice() {
        use self::rand::{Rng, SeedableRng, StdRng};

        for seed in 0..100 {
            println!("{:?}", seed);
            let mut rng = StdRng::from_seed(&[seed]);

            let mut buffer = Buffer::new(1);
            let mut reference_string = String::new();

            for _i in 0..30 {
                let end = rng.gen_range::<usize>(0, buffer.len() + 1);
                let start = rng.gen_range::<usize>(0, end + 1);
                let new_text = RandomCharIter(rng)
                    .take(rng.gen_range(0, 10))
                    .collect::<String>();

                buffer.splice(start..end, new_text.as_str());
                reference_string = [
                    &reference_string[0..start],
                    new_text.as_str(),
                    &reference_string[end..],
                ].concat();
                assert_eq!(buffer.to_string(), reference_string);
            }
        }

        struct RandomCharIter<T: Rng>(T);
        impl<T: Rng> Iterator for RandomCharIter<T> {
            type Item = char;

            fn next(&mut self) -> Option<Self::Item> {
                Some(self.0.gen_range(b'a', b'z' + 1).into())
            }
        }
    }

    #[test]
    fn test_len_for_row() {
        let mut buffer = Buffer::new(1);
        buffer.splice(0..0, "abcd\nefg\nhij");
        buffer.splice(12..12, "kl\nmno");
        buffer.splice(18..18, "\npqrs\n");
        buffer.splice(18..21, "\nPQ");

        assert_eq!(buffer.len_for_row(0), Ok(4));
        assert_eq!(buffer.len_for_row(1), Ok(3));
        assert_eq!(buffer.len_for_row(2), Ok(5));
        assert_eq!(buffer.len_for_row(3), Ok(3));
        assert_eq!(buffer.len_for_row(4), Ok(4));
        assert_eq!(buffer.len_for_row(5), Ok(0));
        assert_eq!(buffer.len_for_row(6), Err(Error::OffsetOutOfRange));
    }

    #[test]
    fn iter_starting_at_row() {
        let mut buffer = Buffer::new(1);
        buffer.splice(0..0, "abcd\nefgh\nij");
        buffer.splice(12..12, "kl\nmno");
        buffer.splice(18..18, "\npqrs");
        buffer.splice(18..21, "\nPQ");

        let iter = buffer.iter_starting_at_row(0);
        assert_eq!(
            String::from_utf16_lossy(&iter.collect::<Vec<u16>>()),
            "abcd\nefgh\nijkl\nmno\nPQrs"
        );

        let iter = buffer.iter_starting_at_row(1);
        assert_eq!(
            String::from_utf16_lossy(&iter.collect::<Vec<u16>>()),
            "efgh\nijkl\nmno\nPQrs"
        );

        let iter = buffer.iter_starting_at_row(2);
        assert_eq!(
            String::from_utf16_lossy(&iter.collect::<Vec<u16>>()),
            "ijkl\nmno\nPQrs"
        );

        let iter = buffer.iter_starting_at_row(3);
        assert_eq!(
            String::from_utf16_lossy(&iter.collect::<Vec<u16>>()),
            "mno\nPQrs"
        );

        let iter = buffer.iter_starting_at_row(4);
        assert_eq!(
            String::from_utf16_lossy(&iter.collect::<Vec<u16>>()),
            "PQrs"
        );

        let iter = buffer.iter_starting_at_row(5);
        assert_eq!(String::from_utf16_lossy(&iter.collect::<Vec<u16>>()), "");
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
    fn fragment_ids() {
        for seed in 0..10 {
            use self::rand::{Rng, SeedableRng, StdRng};
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
        let mut buffer = Buffer::new(1);
        buffer.splice(0..0, "abc");
        let left_anchor = buffer.anchor_before_offset(2).unwrap();
        let right_anchor = buffer.anchor_after_offset(2).unwrap();

        buffer.splice(1..1, "def\n");
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

        buffer.splice(2..3, "");
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

        buffer.splice(5..5, "ghi\n");
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

        buffer.splice(7..9, "");
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
    fn anchors_at_start_and_end() {
        let mut buffer = Buffer::new(1);
        let before_start_anchor = buffer.anchor_before_offset(0).unwrap();
        let after_end_anchor = buffer.anchor_after_offset(0).unwrap();

        buffer.splice(0..0, "abc");
        assert_eq!(buffer.to_string(), "abc");
        assert_eq!(buffer.offset_for_anchor(&before_start_anchor).unwrap(), 0);
        assert_eq!(buffer.offset_for_anchor(&after_end_anchor).unwrap(), 3);

        let after_start_anchor = buffer.anchor_after_offset(0).unwrap();
        let before_end_anchor = buffer.anchor_before_offset(3).unwrap();

        buffer.splice(3..3, "def");
        buffer.splice(0..0, "ghi");
        assert_eq!(buffer.to_string(), "ghiabcdef");
        assert_eq!(buffer.offset_for_anchor(&before_start_anchor).unwrap(), 0);
        assert_eq!(buffer.offset_for_anchor(&after_start_anchor).unwrap(), 3);
        assert_eq!(buffer.offset_for_anchor(&before_end_anchor).unwrap(), 6);
        assert_eq!(buffer.offset_for_anchor(&after_end_anchor).unwrap(), 9);
    }
}
