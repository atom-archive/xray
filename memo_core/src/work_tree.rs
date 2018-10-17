use epoch::{self, DirEntry, Epoch};
use futures::{future, stream, Future, Stream};
use notify_cell::NotifyCell;
use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::io;
use std::path::Path;
use std::rc::Rc;
use time;
use Oid;
use ReplicaId;

pub trait GitProvider {
    fn base_entries(&self, oid: Oid) -> Box<Stream<Item = DirEntry, Error = io::Error>>;
    fn base_text(&self, oid: Oid, path: &Path) -> Box<Future<Item = String, Error = io::Error>>;
}

struct WorkTree {
    epoch: Option<Rc<RefCell<Epoch>>>,
    deferred_ops: Rc<RefCell<HashMap<epoch::Id, Vec<epoch::Operation>>>>,
    lamport_clock: Rc<RefCell<time::Lamport>>,
    git: Rc<GitProvider>,
    updates: NotifyCell<()>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Operation {
    StartEpoch {
        epoch_id: epoch::Id,
        head: Oid,
    },
    EpochOperation {
        epoch_id: epoch::Id,
        operation: epoch::Operation,
    },
}

#[derive(Debug)]
enum Error {
    InvalidOperations,
    EpochError(epoch::Error),
    IoError(io::Error),
}

impl WorkTree {
    pub fn new(
        replica_id: ReplicaId,
        base: Oid,
        ops: Vec<Operation>,
        git: Rc<GitProvider>,
    ) -> Result<(WorkTree, Box<Stream<Item = Operation, Error = Error>>), Error> {
        let mut tree = WorkTree {
            epoch: None,
            deferred_ops: Rc::new(RefCell::new(HashMap::new())),
            lamport_clock: Rc::new(RefCell::new(time::Lamport::new(replica_id))),
            git,
            updates: NotifyCell::new(()),
        };

        let ops = if ops.is_empty() {
            Box::new(tree.reset(base)) as Box<Stream<Item = Operation, Error = Error>>
        } else {
            Box::new(tree.apply_ops(ops)?) as Box<Stream<Item = Operation, Error = Error>>
        };

        Ok((tree, ops))
    }

    pub fn reset(&mut self, head: Oid) -> impl Stream<Item = Operation, Error = Error> {
        let epoch_id = self.lamport_clock.borrow_mut().tick();
        stream::once(Ok(Operation::StartEpoch { epoch_id, head }))
            .chain(self.start_epoch(epoch_id, head))
    }

    pub fn apply_ops(
        &mut self,
        ops: Vec<Operation>,
    ) -> Result<impl Stream<Item = Operation, Error = Error>, Error> {
        let mut cur_epoch_ops = Vec::new();
        let mut epoch_streams = Vec::new();

        for op in ops {
            match op {
                Operation::StartEpoch { epoch_id, head } => {
                    epoch_streams.push(self.start_epoch(epoch_id, head));
                }
                Operation::EpochOperation {
                    epoch_id,
                    operation,
                } => {
                    if let Some(epoch) = self.epoch.clone() {
                        match epoch_id.cmp(&epoch.borrow().id) {
                            Ordering::Less => {}
                            Ordering::Equal => cur_epoch_ops.push(operation),
                            Ordering::Greater => self.defer_epoch_op(epoch_id, operation),
                        }
                    } else {
                        self.defer_epoch_op(epoch_id, operation);
                    }
                }
            }
        }

        if let Some(epoch) = self.epoch.clone() {
            let mut epoch = epoch.borrow_mut();
            let fixup_ops = epoch.apply_ops(cur_epoch_ops, &mut self.lamport_clock.borrow_mut())?;
            let fixup_ops_stream = Box::new(stream::iter_ok(Operation::stamp(epoch.id, fixup_ops)));
            Ok(epoch_streams.into_iter().fold(
                fixup_ops_stream as Box<Stream<Item = Operation, Error = Error>>,
                |acc, stream| Box::new(acc.chain(stream)),
            ))
        } else {
            Err(Error::InvalidOperations)
        }
    }

    fn start_epoch(
        &mut self,
        epoch_id: epoch::Id,
        head: Oid,
    ) -> Box<Stream<Item = Operation, Error = Error>> {
        if self
            .epoch
            .as_ref()
            .map_or(true, |e| epoch_id > e.borrow().id)
        {
            let epoch = Rc::new(RefCell::new(Epoch::new(self.replica_id(), epoch_id, head)));
            if self.epoch.is_none() {
                self.epoch = Some(epoch.clone());
            }
            let cur_epoch = self.epoch.clone().unwrap();
            let deferred_ops = self.deferred_ops.clone();
            let lamport_clock_1 = self.lamport_clock.clone();
            let lamport_clock_2 = self.lamport_clock.clone();

            let epoch_1 = epoch.clone();
            let load_base_entries = self
                .git
                .base_entries(head)
                .map_err(|err| Error::IoError(err))
                .chunks(500)
                .and_then(move |base_entries| {
                    let fixup_ops = epoch_1
                        .borrow_mut()
                        .append_base_entries(base_entries, &mut lamport_clock_1.borrow_mut())?;
                    Ok(stream::iter_ok(Operation::stamp(epoch_id, fixup_ops)))
                })
                .flatten();

            let epoch_2 = epoch.clone();
            let assign_epoch = future::lazy(move || {
                let mut fixup_ops = Vec::new();
                if epoch_id > cur_epoch.borrow().id {
                    cur_epoch.swap(epoch_2.as_ref());
                    if let Some(ops) = deferred_ops.borrow_mut().remove(&epoch_id) {
                        fixup_ops = cur_epoch
                            .borrow_mut()
                            .apply_ops(ops, &mut lamport_clock_2.borrow_mut())?;
                    }
                    deferred_ops.borrow_mut().retain(|id, _| *id > epoch_id);
                }

                Ok(Box::new(stream::iter_ok(Operation::stamp(
                    epoch_id, fixup_ops,
                ))))
            })
            .flatten_stream();

            Box::new(load_base_entries.chain(assign_epoch))
        } else {
            Box::new(stream::empty())
        }
    }

    fn defer_epoch_op(&self, epoch_id: epoch::Id, operation: epoch::Operation) {
        self.deferred_ops
            .borrow_mut()
            .entry(epoch_id)
            .or_insert(Vec::new())
            .push(operation);
    }

    fn replica_id(&self) -> ReplicaId {
        self.lamport_clock.borrow().replica_id
    }
}

impl Operation {
    fn stamp<T>(epoch_id: epoch::Id, operations: T) -> impl Iterator<Item = Operation>
    where
        T: IntoIterator<Item = epoch::Operation>,
    {
        operations
            .into_iter()
            .map(move |operation| Operation::EpochOperation {
                epoch_id,
                operation,
            })
    }
}

impl From<epoch::Error> for Error {
    fn from(error: epoch::Error) -> Self {
        Error::EpochError(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use epoch::CursorEntry;
    use rand::{SeedableRng, StdRng};

    #[test]
    fn test_reset() {
        let mut rng = StdRng::from_seed(&[1]);
        let mut base_tree_clock = &mut time::Lamport::new(999);

        let mut base_tree = Epoch::with_replica_id(999);
        base_tree.mutate(&mut rng, &mut base_tree_clock, 5);

        let mut git = TestGitProvider::new();
        git.commit([0; 20], base_tree.clone());

        base_tree.mutate(&mut rng, &mut base_tree_clock, 5);
        git.commit([1; 20], base_tree.clone());

        base_tree.mutate(&mut rng, &mut base_tree_clock, 5);
        git.commit([2; 20], base_tree.clone());

        let git = Rc::new(git);
        let (mut tree_1, ops_1) = WorkTree::new(1, [0; 20], Vec::new(), git.clone()).unwrap();
        let (mut tree_2, ops_2) =
            WorkTree::new(2, [0; 20], ops_1.collect().wait().unwrap(), git.clone()).unwrap();
        assert!(ops_2.wait().next().is_none());

        assert_eq!(tree_1.dir_entries(), git.tree([0; 20]).dir_entries());
        assert_eq!(tree_2.dir_entries(), git.tree([0; 20]).dir_entries());

        let ops_1 = tree_1.reset([1; 20]).collect().wait().unwrap();
        assert_eq!(tree_1.dir_entries(), git.tree([1; 20]).dir_entries());

        let ops_2 = tree_2.reset([2; 20]).collect().wait().unwrap();
        assert_eq!(tree_2.dir_entries(), git.tree([2; 20]).dir_entries());

        let fixup_ops_1 = tree_1.apply_ops(ops_2).unwrap().collect().wait().unwrap();
        let fixup_ops_2 = tree_2.apply_ops(ops_1).unwrap().collect().wait().unwrap();
        assert!(fixup_ops_1.is_empty());
        assert!(fixup_ops_2.is_empty());
        assert_eq!(tree_1.entries(), tree_2.entries());
    }

    impl WorkTree {
        fn entries(&self) -> Vec<CursorEntry> {
            self.epoch.as_ref().unwrap().borrow().entries()
        }

        fn dir_entries(&self) -> Vec<DirEntry> {
            self.epoch.as_ref().unwrap().borrow().dir_entries()
        }
    }

    struct TestGitProvider {
        commits: HashMap<Oid, Epoch>,
    }

    impl TestGitProvider {
        fn new() -> Self {
            TestGitProvider {
                commits: HashMap::new(),
            }
        }

        fn commit(&mut self, oid: Oid, tree: Epoch) {
            self.commits.insert(oid, tree);
        }

        fn tree(&self, oid: Oid) -> &Epoch {
            self.commits.get(&oid).unwrap()
        }
    }

    impl GitProvider for TestGitProvider {
        fn base_entries(&self, oid: Oid) -> Box<Stream<Item = DirEntry, Error = io::Error>> {
            Box::new(stream::iter_ok(
                self.commits
                    .get(&oid)
                    .unwrap()
                    .entries()
                    .into_iter()
                    .map(|entry| entry.into()),
            ))
        }

        fn base_text(
            &self,
            oid: Oid,
            path: &Path,
        ) -> Box<Future<Item = String, Error = io::Error>> {
            unimplemented!()
        }
    }
}
