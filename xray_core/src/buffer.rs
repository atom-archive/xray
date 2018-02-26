use std::cmp;
use std::collections::HashSet;
use std::iter;
use std::ops::{Add, AddAssign};
use std::sync::Arc;
use super::tree::{self, Tree};
use notify_cell::NotifyCell;

pub type ReplicaId = usize;
type LocalTimestamp = usize;
type LamportTimestamp = usize;

#[derive(Debug)]
pub struct Buffer {
    replica_id: ReplicaId,
    local_clock: LocalTimestamp,
    lamport_clock: LamportTimestamp,
    fragments: Tree<Fragment>,
    pub version: NotifyCell<Version>
}

#[derive(Clone, Copy, Debug)]
pub struct Version(LocalTimestamp);

#[derive(Eq, PartialEq, Debug)]
pub struct Anchor {
    insertion_id: ChangeId,
    offset: usize,
    replica_id: ReplicaId,
    lamport_timestamp: LamportTimestamp
}

pub struct Range<T> {
    pub start: T,
    pub end: T,
    pub reversed: bool
}

pub struct Iter<'a> {
    fragment_cursor: tree::Cursor<'a, Fragment>,
    fragment_offset: usize
}

#[derive(Eq, PartialEq, Debug)]
struct Insertion {
    id: ChangeId,
    start: Anchor,
    text: Text
}

#[derive(Eq, PartialEq, Debug)]
pub struct Text {
    code_units: Vec<u16>,
    newline_offsets: Vec<usize>
}

#[derive(Hash, Eq, PartialEq, Clone, Copy, Debug)]
struct ChangeId {
    replica_id: ReplicaId,
    local_timestamp: LocalTimestamp
}

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
    newline_count: NewlineCount
}

#[derive(Ord, PartialOrd, Eq, PartialEq, Clone, Debug)]
struct FragmentId(Vec<u16>);

#[derive(Ord, PartialOrd, Eq, PartialEq, Clone, Copy, Debug)]
struct NewlineCount(pub usize);

impl Buffer {
    pub fn new(replica_id: ReplicaId) -> Self {
        assert!(replica_id > 0);
        let mut fragments = Tree::<Fragment>::new();

        // Push start sentinel.
        fragments.push(Fragment::new(FragmentId::min_value(), Insertion {
            id: ChangeId { replica_id: 0, local_timestamp: 0 },
            start: Anchor {
                insertion_id: ChangeId { replica_id: 0, local_timestamp: 0},
                offset: 0,
                replica_id: 0,
                lamport_timestamp: 0
            },
            text: Text::new(vec![])
        }));

        Buffer {
            replica_id,
            local_clock: 0,
            lamport_clock: 0,
            fragments,
            version: NotifyCell::new(Version(0))
        }
    }

    pub fn len(&self) -> usize {
        self.fragments.len()
    }

    pub fn to_u16_chars(&self) -> Vec<u16> {
        let mut result = Vec::with_capacity(self.len());
        result.extend(self.iter());
        result
    }

    pub fn iter(&self) -> Iter {
        Iter::new(self)
    }

    pub fn iter_starting_at_row(&self, row: usize) -> Iter {
        Iter::starting_at_row(self, NewlineCount(row))
    }

    pub fn splice<T: Into<Text>>(&mut self, old_range: Range<usize>, new_text: T) {
        let new_text = new_text.into();
        let new_text = if new_text.len() > 0 { Some(new_text) } else { None };
        if new_text.is_some() || old_range.end > old_range.start {
            self.local_clock += 1;
            self.lamport_clock += 1;
            let change_id = ChangeId {
                replica_id: self.replica_id,
                local_timestamp: self.local_clock
            };
            self.fragments = self.splice_fragments(change_id, old_range, new_text);
            self.version.set(Version(self.local_clock));
        }
    }

    fn splice_fragments(&self, change_id: ChangeId, old_range: Range<usize>, mut new_text: Option<Text>) -> Tree<Fragment> {
        let mut cursor = self.fragments.cursor();
        let mut updated_fragments = cursor.build_prefix(&old_range.start);
        let mut inserted_fragments = Vec::new();

        if cursor.prev_item().is_none() {
            inserted_fragments.push(cursor.item().unwrap().clone());
            cursor.next();
        }

        let prev_fragment = cursor.prev_item().unwrap();
        if let Some(cur_fragment) = cursor.item() {
            let (before_range, within_range, after_range) = self.split_fragment(prev_fragment, cur_fragment, cursor.start(), &old_range);
            let insertion = new_text.take().map(|new_text| {
                self.build_insertion(
                    change_id.clone(),
                    before_range.as_ref().unwrap_or(prev_fragment),
                    within_range.as_ref().or(after_range.as_ref()),
                    new_text
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
                inserted_fragments.push(self.build_insertion(change_id, prev_fragment, None, new_text));
            });
        }

        loop {
            let fragment_start = cursor.start();

            if fragment_start >= old_range.end { break; }

            let prev_fragment = cursor.prev_item().unwrap();
            let cur_fragment = cursor.item().unwrap();
            let fragment_end = fragment_start + cur_fragment.len();

            if old_range.end < fragment_end {
                let (_, within_range, after_range) = self.split_fragment(prev_fragment, cur_fragment, fragment_start, &old_range);
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
        updated_fragments
    }

    fn split_fragment(&self, prev_fragment: &Fragment, fragment: &Fragment, fragment_start: usize, range: &Range<usize>) -> (Option<Fragment>, Option<Fragment>, Option<Fragment>) {
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
            suffix.start_offset = prefix.start_offset + cmp::max(range.start, fragment_start) - fragment_start;
            prefix.end_offset = suffix.start_offset;
            prefix.id = FragmentId::between(&prev_fragment.id, &suffix.id);
            within_range = Some(suffix);
        }

        if range.start > fragment_start {
            before_range = Some(prefix);
        }

        (before_range, within_range, after_range)
    }

    fn build_insertion(&self, change_id: ChangeId, prev_fragment: &Fragment, next_fragment: Option<&Fragment>, text: Text) -> Fragment {
        let new_fragment_id = FragmentId::between(
            &prev_fragment.id,
            next_fragment.map(|f| &f.id).unwrap_or(&FragmentId::max_value())
        );

        Fragment::new(new_fragment_id, Insertion {
            id: change_id,
            start: Anchor {
                insertion_id: prev_fragment.insertion.id,
                offset: prev_fragment.end_offset,
                replica_id: self.replica_id,
                lamport_timestamp: self.lamport_clock
            },
            text
        })
    }
}

impl<T> Range<T> {
    pub fn new(start: T, end: T) -> Self {
        Self { start, end, reversed: false }
    }
}

impl<'a> Iter<'a> {
    fn new(buffer: &'a Buffer) -> Self {
        let mut fragment_cursor = buffer.fragments.cursor();
        fragment_cursor.seek(&0);
        Self {
            fragment_cursor,
            fragment_offset: 0
        }
    }

    fn starting_at_row(buffer: &'a Buffer, target_row: NewlineCount) -> Self {
        let mut fragment_cursor = buffer.fragments.cursor();
        fragment_cursor.seek(&target_row);

        let mut fragment_offset = 0;
        if let Some(fragment) = fragment_cursor.item() {
            let target_row = target_row.0;
            let fragment_start_row = fragment_cursor.start::<NewlineCount>().0;
            if target_row != fragment_start_row {
                let target_row_within_fragment = target_row - fragment_start_row - 1;
                fragment_offset = fragment.insertion.text.newline_offsets[target_row_within_fragment] + 1;
            }
        }

        Self {
            fragment_cursor,
            fragment_offset
        }
    }
}

impl<'a> Iterator for Iter<'a> {
    type Item = u16;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(fragment) = self.fragment_cursor.item() {
            if let Some(c) = fragment.get_code_unit(self.fragment_offset) {
                self.fragment_offset += 1;
                return Some(c)
            }
        }

        loop {
            self.fragment_cursor.next();
            if let Some(fragment) = self.fragment_cursor.item() {
                if let Some(c) = fragment.get_code_unit(0) {
                    self.fragment_offset = 1;
                    return Some(c)
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
        let newline_offsets = code_units.iter().enumerate().filter_map(|(offset, c)| {
            if *c == (b'\n' as u16) {
                Some(offset)
            } else {
                None
            }
        }).collect();

        Self { code_units, newline_offsets }
    }

    fn len(&self) -> usize {
        self.code_units.len()
    }

    fn newline_count_in_range(&self, start: usize, end: usize) -> NewlineCount {
        let newlines_start = find_insertion_index(&self.newline_offsets, &start);
        let newlines_end = find_insertion_index(&self.newline_offsets, &end);
        NewlineCount(newlines_end - newlines_start)
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

impl Fragment {
    fn new(id: FragmentId, ins: Insertion) -> Self {
        let end_offset = ins.text.len();
        Self {
            id,
            insertion: Arc::new(ins),
            start_offset: 0,
            end_offset,
            deletions: HashSet::new()
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
}

impl tree::Item for Fragment {
    type Summary = FragmentSummary;

    fn summarize(&self) -> Self::Summary {
        if self.is_visible() {
            FragmentSummary {
                extent: self.len(),
                newline_count: self.insertion.text.newline_count_in_range(
                    self.start_offset,
                    self.end_offset
                )
            }
        } else {
            FragmentSummary {
                extent: 0,
                newline_count: NewlineCount(0)
            }
        }
    }
}

impl<'a> AddAssign<&'a FragmentSummary> for FragmentSummary {
    fn add_assign(&mut self, other: &Self) {
        self.extent += other.extent;
        self.newline_count += other.newline_count;
    }
}

impl Default for FragmentSummary {
    fn default() -> Self {
        FragmentSummary {
            extent: 0,
            newline_count: NewlineCount(0)
        }
    }
}

impl tree::Dimension for usize {
    type Summary = FragmentSummary;

    fn from_summary(summary: &Self::Summary) -> Self {
        summary.extent
    }

    #[inline]
    fn is_strictly_increasing() -> bool {
        true
    }
}

impl tree::Dimension for NewlineCount {
    type Summary = FragmentSummary;

    fn from_summary(summary: &Self::Summary) -> Self {
        summary.newline_count
    }

    #[inline]
    fn is_strictly_increasing() -> bool {
        false
    }
}

impl<'a> Add<&'a Self> for NewlineCount {
    type Output = NewlineCount;

    fn add(self, other: &'a Self) -> Self::Output {
        NewlineCount(self.0 + other.0)
    }
}

impl AddAssign for NewlineCount {
    fn add_assign(&mut self, other: Self) {
        self.0 += other.0;
    }
}

impl FragmentId {
    fn min_value() -> Self {
        FragmentId(vec![0 as u16])
    }

    fn max_value() -> Self {
        FragmentId(vec![u16::max_value()])
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
                break
            } else {
                new_entries.push(l);
            }
        }

        FragmentId(new_entries)
    }
}

fn find_insertion_index<T: Ord>(v: &Vec<T>, x: &T) -> usize {
    match v.binary_search(x) {
        Ok(index) => index,
        Err(index) => index
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    impl Buffer {
        fn to_string(&self) -> String {
            String::from_utf16_lossy(self.iter().collect::<Vec<u16>>().as_slice())
        }
    }

    #[test]
    fn splice() {
        let mut buffer = Buffer::new(1);
        buffer.splice(Range::new(0, 0), "abc");
        assert_eq!(buffer.to_string(), "abc");
        buffer.splice(Range::new(3, 3), "def");
        assert_eq!(buffer.to_string(), "abcdef");
        buffer.splice(Range::new(0, 0), "ghi");
        assert_eq!(buffer.to_string(), "ghiabcdef");
        buffer.splice(Range::new(5, 5), "jkl");
        assert_eq!(buffer.to_string(), "ghiabjklcdef");
        buffer.splice(Range::new(6, 7), "");
        assert_eq!(buffer.to_string(), "ghiabjlcdef");
        buffer.splice(Range::new(4, 9), "mno");
        assert_eq!(buffer.to_string(), "ghiamnoef");
    }

    #[test]
    fn random_splice() {
        use rand::{Rng, SeedableRng, StdRng};

        for seed in 0..100 {
            println!("{:?}", seed);
            let mut rng = StdRng::from_seed(&[seed]);

            let mut buffer = Buffer::new(1);
            let mut reference_string = String::new();

            for _i in 0..30 {
                let end = rng.gen_range::<usize>(0, buffer.len() + 1);
                let start = rng.gen_range::<usize>(0, end + 1);
                let new_text = RandomCharIter(rng).take(rng.gen_range(0, 10)).collect::<String>();

                buffer.splice(Range::new(start, end), new_text.as_str());
                reference_string = [&reference_string[0..start], new_text.as_str(), &reference_string[end..]].concat();
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
    fn iter_starting_at_row() {
        let mut buffer = Buffer::new(1);
        buffer.splice(Range::new(0, 0), "abcd\nefgh\nij");
        buffer.splice(Range::new(12, 12), "kl\nmno");
        buffer.splice(Range::new(18, 18), "\npqrs");
        buffer.splice(Range::new(18, 21), "\nPQ");

        let iter = buffer.iter_starting_at_row(0);
        assert_eq!(String::from_utf16_lossy(&iter.collect::<Vec<u16>>()), "abcd\nefgh\nijkl\nmno\nPQrs");

        let iter = buffer.iter_starting_at_row(1);
        assert_eq!(String::from_utf16_lossy(&iter.collect::<Vec<u16>>()), "efgh\nijkl\nmno\nPQrs");

        let iter = buffer.iter_starting_at_row(2);
        assert_eq!(String::from_utf16_lossy(&iter.collect::<Vec<u16>>()), "ijkl\nmno\nPQrs");

        let iter = buffer.iter_starting_at_row(3);
        assert_eq!(String::from_utf16_lossy(&iter.collect::<Vec<u16>>()), "mno\nPQrs");

        let iter = buffer.iter_starting_at_row(4);
        assert_eq!(String::from_utf16_lossy(&iter.collect::<Vec<u16>>()), "PQrs");

        let iter = buffer.iter_starting_at_row(5);
        assert_eq!(String::from_utf16_lossy(&iter.collect::<Vec<u16>>()), "");
    }

    #[test]
    fn text_newline_count() {
        let text = Text::from("abc\ndefgh\nijklm\nopq");
        assert_eq!(text.newline_count_in_range(3, 15), NewlineCount(2));
        assert_eq!(text.newline_count_in_range(3, 16), NewlineCount(3));
        assert_eq!(text.newline_count_in_range(4, 16), NewlineCount(2));
    }

    #[test]
    fn fragment_ids() {
        for seed in 0..10 {
            use rand::{Rng, SeedableRng, StdRng};
            let mut rng = StdRng::from_seed(&[seed]);

            let mut ids = vec![FragmentId(vec![0]), FragmentId(vec![4])];
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
}
