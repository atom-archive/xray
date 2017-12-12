use std::collections::HashSet;
use std::sync::Arc;
use super::tree::{self, Tree};

type ReplicaId = usize;
type LocalTimestamp = usize;
type LamportTimestamp = usize;

pub struct Buffer {
    replica_id: ReplicaId,
    local_clock: LocalTimestamp,
    lamport_clock: LamportTimestamp,
    fragments: Tree<Fragment>
}

#[derive(Eq, PartialEq, Debug)]
pub struct Position {
    insertion_id: SpliceId,
    offset: usize,
    replica_id: ReplicaId,
    lamport_timestamp: LamportTimestamp
}

#[derive(Eq, PartialEq, Debug)]
struct Insertion {
    id: SpliceId,
    start: Position,
    text: Text
}

#[derive(Eq, PartialEq, Debug)]
struct Text {
    characters: Vec<u16>,
    newline_offsets: Vec<usize>
}

#[derive(Hash, Eq, PartialEq, Clone, Copy, Debug)]
struct SpliceId {
    replica_id: ReplicaId,
    local_timestamp: LocalTimestamp
}

#[derive(Eq, PartialEq, Clone, Debug)]
struct Fragment {
    id: FragmentId,
    insertion: Arc<Insertion>,
    start_offset: usize,
    end_offset: usize,
    deletions: HashSet<SpliceId>,
}

#[derive(Eq, PartialEq, Clone, Debug)]
struct FragmentSummary {
    max_id: Option<FragmentId>,
    extent: usize,
    newline_count: usize
}

#[derive(Eq, PartialEq, Clone, Debug)]
struct FragmentId (Vec<u16>);

impl Buffer {
    fn new(replica_id: ReplicaId) {
        assert!(replica_id > 0);
        let mut fragments = Tree::<Fragment>::new();

        // Push start sentinel.
        fragments.push(Fragment::new(FragmentId::min_value(), Insertion {
            id: SpliceId { replica_id: 0, local_timestamp: 0 },
            start: Position {
                insertion_id: SpliceId { replica_id: 0, local_timestamp: 0},
                offset: 0,
                replica_id: 0,
                lamport_timestamp: 0
            },
            text: Text::new(vec![])
        }));
    }
}

impl Text {
    fn new(characters: Vec<u16>) -> Self {
        let newline_offsets = characters.iter().enumerate().filter_map(|(offset, c)| {
            if *c == (b'\n' as u16) {
                Some(offset)
            } else {
                None
            }
        }).collect();

        Self { characters, newline_offsets }
    }

    fn len(&self) -> usize {
        self.characters.len()
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
}

impl tree::Item for Fragment {
    type Summary = FragmentSummary;

    fn summarize(&self) -> Self::Summary {
        FragmentSummary {
            max_id: Some(self.id.clone()),
            extent: self.end_offset - self.start_offset,
            newline_count: self.insertion.text.newline_count_in_range(self.start_offset, self.end_offset)
        }
    }
}

impl tree::Summary for FragmentSummary {
    fn accumulate(&mut self, other: &Self) {
        self.extent += other.extent;
        self.newline_count += other.newline_count;
    }
}

impl Default for FragmentSummary {
    fn default() -> Self {
        FragmentSummary {
            max_id: None,
            extent: 0,
            newline_count: 0
        }
    }
}

impl FragmentId {
    fn min_value() -> Self {
        FragmentId(vec![0 as u16])
    }

    fn max_value() -> Self {
        FragmentId(vec![u16::max_value()])
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

    #[test]
    fn text() {
        let text = Text::from("The\nQuick\nBrown\nFox");
        assert_eq!(text.newline_count_in_range(3, 15), 2);
        assert_eq!(text.newline_count_in_range(3, 16), 3);
        assert_eq!(text.newline_count_in_range(4, 16), 2);
    }
}
