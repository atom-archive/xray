use std::cmp;
use std::collections::HashSet;
use std::iter;
use std::ops::{AddAssign, Range};
use std::sync::Arc;
use super::tree::{self, Tree};

type ReplicaId = usize;
type LocalTimestamp = usize;
type LamportTimestamp = usize;

#[derive(Debug)]
pub struct Buffer {
    replica_id: ReplicaId,
    local_clock: LocalTimestamp,
    lamport_clock: LamportTimestamp,
    fragments: Tree<Fragment>
}

#[derive(Eq, PartialEq, Debug)]
pub struct Position {
    insertion_id: ChangeId,
    offset: usize,
    replica_id: ReplicaId,
    lamport_timestamp: LamportTimestamp
}

pub struct Iter<'a> {
    buffer: &'a Buffer,
    fragment_iter: tree::Iter<'a, Fragment>,
    fragment: Option<&'a Fragment>,
    fragment_offset: usize
}

#[derive(Eq, PartialEq, Debug)]
struct Insertion {
    id: ChangeId,
    start: Position,
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
    newline_count: usize
}

#[derive(Ord, PartialOrd, Eq, PartialEq, Clone, Debug)]
struct FragmentId(Vec<u16>);

#[derive(Eq, PartialEq, Clone, Debug)]
struct Offset(usize);

impl Buffer {
    pub fn new(replica_id: ReplicaId) -> Self {
        assert!(replica_id > 0);
        let mut fragments = Tree::<Fragment>::new();

        // Push start sentinel.
        fragments.push(Fragment::new(FragmentId::min_value(), Insertion {
            id: ChangeId { replica_id: 0, local_timestamp: 0 },
            start: Position {
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
            fragments
        }
    }

    pub fn iter(&self) -> Iter {
        Iter {
            buffer: self,
            fragment_iter: self.fragments.iter(),
            fragment: None,
            fragment_offset: 0
        }
    }

    pub fn edit<T: Into<Text>>(&mut self, old_range: Range<usize>, new_text: T) {
        let new_text = new_text.into();
        let new_text = if new_text.len() > 0 { Some(new_text) } else { None };
        if new_text.is_some() || old_range.end > old_range.start {
            self.local_clock += 1;
            self.lamport_clock += 1;
            let change_id = ChangeId {
                replica_id: self.replica_id,
                local_timestamp: self.local_clock
            };
            self.fragments = self.edit_fragments(change_id, old_range, new_text);
        }
    }

    fn edit_fragments(&self, change_id: ChangeId, old_range: Range<usize>, mut new_text: Option<Text>) -> Tree<Fragment> {
        let mut cursor = self.fragments.cursor();
        let mut updated_fragments = cursor.build_prefix(&old_range.start);
        let mut inserted_fragments = Vec::new();

        if let (None, Some(cur_fragment), _) = cursor.peek() {
            inserted_fragments.push(cur_fragment.clone());
            cursor.next();
        }

        let advance_cursor = {
            let (prev_fragment, cur_fragment, summary) = cursor.next();
            let prev_fragment = prev_fragment.unwrap();

            if let Some(cur_fragment) = cur_fragment {
                let (before_range, within_range, after_range) = self.split_fragment(prev_fragment, cur_fragment, summary.extent, old_range);
                let insertion = new_text.take().map(|new_text| {
                    self.build_insertion(
                        change_id.clone(),
                        before_range.as_ref().unwrap_or(prev_fragment),
                        within_range.as_ref().or(after_range.as_ref()),
                        new_text
                    )
                });

                let did_split = before_range.is_some() || after_range.is_some();
                before_range.map(|fragment| inserted_fragments.push(fragment));
                insertion.map(|fragment| inserted_fragments.push(fragment));
                within_range.map(|mut fragment| {
                    fragment.deletions.insert(change_id.clone());
                    inserted_fragments.push(fragment);
                });
                after_range.map(|fragment| inserted_fragments.push(fragment));
                did_split
            } else {
                new_text.take().map(|new_text| {
                    inserted_fragments.push(self.build_insertion(change_id, prev_fragment, cur_fragment, new_text));
                });
                false
            }
        };

        if advance_cursor {
            cursor.next();
        }


        // TODO: Handle deletion
        // while cursor.peek().2.extent < old_range.end {
        //     cursor.next();
        // }

        updated_fragments.extend(inserted_fragments);
        updated_fragments.push_tree(cursor.build_suffix());
        updated_fragments
    }

    fn split_fragment(&self, prev_fragment: &Fragment, fragment: &Fragment, fragment_start: usize, range: Range<usize>) -> (Option<Fragment>, Option<Fragment>, Option<Fragment>) {
        let fragment_end = fragment_start + fragment.len();
        let mut prefix = fragment.clone();
        let mut before_range = None;
        let mut within_range = None;
        let mut after_range = None;

        if range.end < fragment_end {
            let mut suffix = prefix.clone();
            suffix.start_offset = range.end - fragment_start;
            prefix.end_offset = suffix.start_offset;
            prefix.id = FragmentId::between(&prev_fragment.id, &suffix.id);
            after_range = Some(suffix);
        }

        if range.start < range.end {
            let mut suffix = prefix.clone();
            suffix.start_offset = cmp::max(range.start, fragment_start) - fragment_start;
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
            start: Position {
                insertion_id: prev_fragment.insertion.id,
                offset: prev_fragment.end_offset,
                replica_id: self.replica_id,
                lamport_timestamp: self.lamport_clock
            },
            text
        })
    }

    fn is_fragment_visible(&self, fragment: &Fragment) -> bool {
        fragment.deletions.is_empty()
    }
}

impl<'a> Iterator for Iter<'a> {
    type Item = u16;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(fragment) = self.fragment {
            self.fragment_offset += 1;
            if let Some(c) = fragment.get_code_unit(self.fragment_offset) {
                return Some(c)
            }
        }

        while let Some(fragment) = self.fragment_iter.next() {
            if self.buffer.is_fragment_visible(fragment) {
                if let Some(result) = fragment.get_code_unit(0) {
                    self.fragment_offset = 0;
                    self.fragment = Some(fragment);
                    return Some(result);
                }
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

    fn newline_count_in_range(&self, start: usize, end: usize) -> usize {
        let newlines_start = find_insertion_index(&self.newline_offsets, &start);
        let newlines_end = find_insertion_index(&self.newline_offsets, &end);
        newlines_end - newlines_start
    }
}

impl<'a> From<&'a str> for Text {
    fn from(s: &'a str) -> Self {
        Self::new(s.encode_utf16().collect())
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
        self.end_offset - self.start_offset
    }
}

impl tree::Item for Fragment {
    type Summary = FragmentSummary;

    fn summarize(&self) -> Self::Summary {
        FragmentSummary {
            extent: self.end_offset - self.start_offset,
            newline_count: self.insertion.text.newline_count_in_range(self.start_offset, self.end_offset)
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
            newline_count: 0
        }
    }
}

impl tree::Dimension for usize {
    type Summary = FragmentSummary;

    fn from_summary(summary: &Self::Summary) -> Self {
        summary.extent
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
        fn get_text(&self) -> String {
            String::from_utf16_lossy(self.iter().collect::<Vec<u16>>().as_slice())
        }
    }

    #[test]
    fn edit() {
        let mut buffer = Buffer::new(1);
        buffer.edit(0..0, "abc");
        assert_eq!(buffer.get_text(), "abc");
        buffer.edit(3..3, "def");
        assert_eq!(buffer.get_text(), "abcdef");
        buffer.edit(0..0, "ghi");
        assert_eq!(buffer.get_text(), "ghiabcdef");
        buffer.edit(5..5, "jkl");
        assert_eq!(buffer.get_text(), "ghiabjklcdef");
        buffer.edit(6..7, "");
        assert_eq!(buffer.get_text(), "ghiabjlcdef");
    }

    #[test]
    fn text_newline_count() {
        let text = Text::from("abc\ndefgh\nijklm\nopq");
        assert_eq!(text.newline_count_in_range(3, 15), 2);
        assert_eq!(text.newline_count_in_range(3, 16), 3);
        assert_eq!(text.newline_count_in_range(4, 16), 2);
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
