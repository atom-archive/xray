use btree::{Dimension, Item, SeekBias, Tree};
#[cfg(test)]
use rand::Rng;
use std::collections::{btree_map::Entry, BTreeMap};
use std::ffi::{OsStr, OsString};
use std::ops::{Add, AddAssign};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct Index {
    root: File,
}

#[derive(Clone, Debug)]
enum File {
    Dir(BTreeMap<OsString, File>),
    File,
}

pub enum Error {
    InvalidPath,
}

impl Index {
    pub fn new() -> Self {
        Self {
            root: File::Dir(BTreeMap::new()),
        }
    }

    pub fn create_dir_all(&mut self, path: &Path) -> Result<(PathBuf, PathBuf), Error> {
        let mut prefix = PathBuf::new();
        let mut suffix = PathBuf::new();
        let mut parent_exists = true;
        let mut parent = &mut self.root;

        for component in path.components() {
            match component {
                Component::Normal(name) => {
                    match { parent } {
                        File::Dir(entries) => {
                            if !entries.contains_key(name) {
                                entries.insert(name.into(), File::Dir(BTreeMap::new()));
                                parent_exists = false;
                            }

                            parent = entries.get_mut(name).unwrap();
                        }
                        File::File => return Err(Error::InvalidPath),
                    }

                    if parent_exists {
                        prefix.push(name);
                    } else {
                        suffix.push(name);
                    }
                }
                _ => return Err(Error::InvalidPath),
            }
        }

        Ok((prefix, suffix))
    }
}
