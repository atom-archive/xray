use std::collections::HashSet;
use std::sync::Arc;
use super::tree::{self, Tree};

type ReplicaId = u32;
type SequenceNumber = u32;

pub struct Buffer {
    replica_id: ReplicaId,
    countuence_number: SequenceNumber,
    fragments: Tree<Fragment>
}

#[derive(Eq, PartialEq, Debug)]
struct Insertion {
    id: SpliceId,
    text: Vec<u16>,
    newline_offsets: Vec<usize>
}

#[derive(Hash, Eq, PartialEq, Clone, Copy, Debug)]
struct SpliceId {
    replica: ReplicaId,
    count: SequenceNumber
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
    min_id: FragmentId,
    extent: usize,
    newline_count: usize
}

#[derive(Eq, PartialEq, Clone, Debug)]
struct FragmentId (Vec<u16>);

trait FindInsertionIndex<T: Ord> {
    fn find_insertion_index(&self, x: &T) -> usize;
}

impl Buffer {
    fn new(replica_id: ReplicaId) {
        assert!(replica_id > 0);
        let mut fragments = Tree::<Fragment>::new();

        // Start sentinel.
        fragments.push(Fragment::new(FragmentId::min_value(), Insertion {
            id: SpliceId { replica: 0, count: 0 },
            text: vec![],
            newline_offsets: vec![]
        }));

        // End sentinel.
        fragments.push(Fragment::new(FragmentId::max_value(), Insertion {
            id: SpliceId { replica: 0, count: 1 },
            text: vec![],
            newline_offsets: vec![]
        }));
    }
}

impl Insertion {
    fn new(id: SpliceId, text: Vec<u16>) -> Self {
        let newline_offsets = text.iter().enumerate().filter_map(|(offset, c)| {
            if *c == (b'\n' as u16) {
                Some(offset)
            } else {
                None
            }
        }).collect();

        Self {
            id,
            text,
            newline_offsets
        }
    }

    fn with_string(id: SpliceId, s: &str) -> Self {
        Self::new(id, s.encode_utf16().collect())
    }

    fn len(&self) -> usize {
        self.text.len()
    }

    fn newline_count_in_offset_range(&self, start: usize, end: usize) -> usize {
        let newlines_start = self.newline_offsets.find_insertion_index(&start);
        let newlines_end = self.newline_offsets.find_insertion_index(&end);
        newlines_end - newlines_start
    }
}

impl Fragment {
    fn new(id: FragmentId, ins: Insertion) -> Self {
        let end_offset = ins.len();
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
            min_id: self.id.clone(),
            extent: self.end_offset - self.start_offset,
            newline_count: self.insertion.newline_count_in_offset_range(self.start_offset, self.end_offset)
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
            min_id: FragmentId::min_value(),
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

impl<T: Ord> FindInsertionIndex<T> for Vec<T> {
    fn find_insertion_index(&self, x: &T) -> usize {
        match self.binary_search(x) {
            Ok(index) => index,
            Err(index) => index
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insertion__newline_counting() {
        let id = SpliceId { replica: 1, count: 0 };
        let ins = Insertion::with_string(id, "The\nQuick\nBrown\nFox");
        assert_eq!(ins.newline_count_in_offset_range(3, 15), 2);
        assert_eq!(ins.newline_count_in_offset_range(3, 16), 3);
        assert_eq!(ins.newline_count_in_offset_range(4, 16), 2);
    }
}
