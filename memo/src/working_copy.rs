use std::path::{Path, PathBuf};
use index::Index;
use patch::{Operation, Patch};
use std::ffi::{OsStr, OsString};
use ReplicaId;

pub struct WorkingCopy {
    index: Index,
    changes: Patch,
}

impl WorkingCopy {
    pub fn new<T>(replica_id: ReplicaId, base_tree: Index) -> Self {
        Self {
            index: base_tree,
            changes: Patch::new(replica_id),
        }
    }

    pub fn create_dir_all(&mut self, path: &Path) -> Vec<Operation> {
        unimplemented!()
    }

    pub fn apply_ops(&mut self, ops: Vec<Operation>) {
        unimplemented!()
    }

    pub fn paths(&self) -> Vec<PathBuf> {
        unimplemented!()
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

        let ops_1 = tree_1.create_dir_all(&PathBuf::from("a/b/c/d"));
        let ops_2 = tree_2.create_dir_all(&PathBuf::from("a/b/e/f"));
        tree_1.apply_ops(ops_2);
        tree_2.apply_ops(ops_1);

        assert_eq!(tree_1.paths(), tree_2.paths());
    }
}
