use std::collections::HashSet;
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
struct Text {
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

#[derive(Eq, PartialEq, Clone, Debug)]
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
            fragment_iter: self.fragments.iter(),
            fragment: None,
            fragment_offset: 0
        }
    }

    pub fn edit<T: Into<Text>>(&mut self, old_range: Range<usize>, new_text: T) {
        let new_text = new_text.into();
        let mut new_text = if new_text.len() > 0 { Some(new_text) } else { None };

        let fragments = {
            let mut cursor = self.fragments.cursor();
            let mut new_fragments = cursor.build_prefix(&old_range.start);
            let mut inserted_fragments = Vec::new();

            loop {
                let (prev_fragment, cur_fragment, summary) = cursor.next();

                // println!("==========\n{:?}\n{:?}\n{:?}", prev_fragment, cur_fragment, summary);

                if let Some(prev_fragment) = prev_fragment {
                    if new_text.is_some() {
                        let new_fragment_id = match cur_fragment {
                            Some(ref cur_fragment) => FragmentId::between(&prev_fragment.id, &cur_fragment.id),
                            None => FragmentId::between(&prev_fragment.id, &FragmentId::max_value())
                        };

                        inserted_fragments.push(Fragment::new(new_fragment_id, Insertion {
                            id: ChangeId {
                                replica_id: self.replica_id,
                                local_timestamp: self.local_clock
                            },
                            start: Position {
                                insertion_id: prev_fragment.insertion.id,
                                offset: prev_fragment.end_offset,
                                replica_id: self.replica_id,
                                lamport_timestamp: self.lamport_clock
                            },
                            text: new_text.take().unwrap()
                        }));
                    }

                    debug_assert!(summary.extent <= old_range.end);

                    // println!("{:?} {:?}", summary.extent, old_range.end);

                    if summary.extent == old_range.end {
                        break;
                    }
                } else {
                    debug_assert!(cur_fragment.is_some());
                    continue;
                }
            }

            new_fragments.extend(inserted_fragments);
            new_fragments.push(cursor.build_suffix());
            new_fragments
        };

        self.fragments = fragments;
        self.local_clock += 1;
        self.lamport_clock += 1;
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

        if let Some(fragment) = self.fragment_iter.next() {
            let result = fragment.get_code_unit(0);
            self.fragment_offset = 0;
            self.fragment = Some(fragment);
            result
        } else {
            None
        }
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
        self.insertion.text.code_units.get(self.start_offset + offset).cloned()
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
        let mut last_level = 0;
        for (left_value, right_value) in left.0.iter().zip(right.0.iter()) {
            if right_value - left_value > 1 { break }
            last_level += 1
        }

        let mut new_values = Vec::with_capacity(last_level + 1);
        new_values.extend(left.0.iter().take(last_level).cloned());

        let lower_bound = *left.0.get(last_level).unwrap_or(&u16::min_value());
        let upper_bound = *right.0.get(last_level).unwrap_or(&u16::max_value());
        new_values.push(lower_bound + ((upper_bound - lower_bound) / 2));

        FragmentId(new_values)
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
    fn buffer_edit() {
        let mut buffer = Buffer::new(1);
        buffer.edit(0..0, "abcdef");
        assert_eq!(buffer.get_text(), "abcdef");
    }

    #[test]
    fn text_newline_count() {
        let text = Text::from("abc\ndefgh\nijklm\nopq");
        assert_eq!(text.newline_count_in_range(3, 15), 2);
        assert_eq!(text.newline_count_in_range(3, 16), 3);
        assert_eq!(text.newline_count_in_range(4, 16), 2);
    }
}
