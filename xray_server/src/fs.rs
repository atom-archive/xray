use futures::{Future, Stream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use walkdir::WalkDir;
use xray_core::fs::{self, Dir, File};
use xray_core::notify_cell::NotifyCell;

pub struct Tree {
    path: PathBuf,
    root: Dir,
    updates: Arc<NotifyCell<()>>
}

impl Tree {
    pub fn new<T: Into<PathBuf>>(path: T) -> Self {
        let path = path.into();
        let root = Dir::new(false);
        let updates = Arc::new(NotifyCell::new(()));
        Self::populate(path.clone(), root.clone(), updates.clone());
        Self { path, root, updates }
    }

    fn populate(path: PathBuf, root: Dir, updates: Arc<NotifyCell<()>>) {
        thread::spawn(move || {
            let mut stack = vec![root];

            let entries = WalkDir::new(path.clone())
                .follow_links(true)
                .into_iter()
                .skip(1)
                .filter_map(|e| e.ok());

            for entry in entries {
                stack.truncate(entry.depth());

                let file_type = entry.file_type();
                let file_name = entry.file_name();

                if file_type.is_dir() {
                    let dir = Dir::new(file_type.is_symlink());
                    stack.last_mut().unwrap().add_dir(file_name, dir.clone());
                    stack.push(dir);
                } else if file_type.is_file() {
                    let file = File::new(file_type.is_symlink());
                    stack.last_mut().unwrap().add_file(file_name, file);
                }
                updates.set(());
            }
        });
    }
}

impl fs::Tree for Tree {
    fn path(&self) -> &Path {
        &self.path
    }

    fn root(&self) -> &Dir {
        &self.root
    }

    fn updates(&self) -> Box<Stream<Item = (), Error = ()>> {
        Box::new(self.updates.observe())
    }
}
