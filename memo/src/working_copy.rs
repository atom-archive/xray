use index::{self, Index};
use patch::{self, Operation, Patch};
use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::path::{Component, Path, PathBuf};
use ReplicaId;

pub struct WorkingCopy {
    index: Index,
    changes: Patch,
}

#[derive(Debug)]
pub enum Error {
    InvalidPath,
}

impl WorkingCopy {
    pub fn new(replica_id: ReplicaId, base_tree: Index) -> Self {
        Self {
            index: base_tree,
            changes: Patch::new(replica_id),
        }
    }

    pub fn create_dir_all(&mut self, path: &Path) -> Result<Vec<Operation>, Error> {
        let mut operations = Vec::new();

        let (prefix, suffix) = self.index.create_dir_all(path)?;
        let (mut parent_id, op) = self.changes.file_id(&prefix)?;
        operations.extend(op);
        for name in suffix.components() {
            let (dir_id, op) = self.changes.new_directory();
            operations.push(op);
            match name {
                Component::Normal(name) => {
                    operations.push(self.changes.rename(dir_id, parent_id, name));
                }
                _ => return Err(Error::InvalidPath),
            }

            parent_id = dir_id;
        }

        Ok(operations)
    }

    pub fn apply_ops(&mut self, ops: Vec<Operation>) {
        unimplemented!()
    }

    pub fn paths(&self) -> Vec<PathBuf> {
        unimplemented!()
    }
}

impl From<patch::Error> for Error {
    fn from(error: patch::Error) -> Self {
        match error {
            patch::Error::InvalidPath => Error::InvalidPath,
        }
    }
}

impl From<index::Error> for Error {
    fn from(error: index::Error) -> Self {
        match error {
            index::Error::InvalidPath => Error::InvalidPath,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_replication_basic() {
        let mut index = Index::new();
        index.create_dir_all(&PathBuf::from("a/b"));
        let mut tree_1 = WorkingCopy::new(1, index.clone());
        let mut tree_2 = WorkingCopy::new(1, index.clone());

        let ops_1 = tree_1.create_dir_all(&PathBuf::from("a/b/c/d")).unwrap();
        let ops_2 = tree_2.create_dir_all(&PathBuf::from("a/b/e/f")).unwrap();
        tree_1.apply_ops(ops_2);
        tree_2.apply_ops(ops_1);

        assert_eq!(tree_1.paths(), tree_2.paths());
    }
}
