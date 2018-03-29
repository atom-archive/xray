use futures::Stream;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use ignore::WalkBuilder;
use xray_core::fs;
use xray_core::notify_cell::NotifyCell;

pub struct Tree {
    path: PathBuf,
    root: fs::Entry,
    updates: Arc<NotifyCell<()>>
}

impl Tree {
    pub fn new<T: Into<PathBuf>>(path: T) -> Self {
        let path = path.into();
        let root = fs::Entry::dir(0, OsString::from(path.file_name().unwrap()), false, false);
        let updates = Arc::new(NotifyCell::new(()));
        Self::populate(path.clone(), root.clone(), updates.clone());
        Self { path, root, updates }
    }

    fn populate(path: PathBuf, root: fs::Entry, updates: Arc<NotifyCell<()>>) {
        thread::spawn(move || {
            let mut stack = vec![root];
            let mut next_entry_id = 1;

            let entries = WalkBuilder::new(path.clone())
                .follow_links(true)
                .include_ignored(true)
                .build()
                .skip(1)
                .filter_map(|e| e.ok());

            for entry in entries {
                stack.truncate(entry.depth());

                let file_type = entry.file_type().unwrap();
                let file_name = entry.file_name();

                if file_type.is_dir() {
                    let dir = fs::Entry::dir(next_entry_id, OsString::from(file_name), file_type.is_symlink(), entry.ignored());
                    stack.last_mut().unwrap().insert(dir.clone()).unwrap();
                    stack.push(dir);
                } else if file_type.is_file() {
                    let file = fs::Entry::file(next_entry_id, OsString::from(file_name), file_type.is_symlink(), entry.ignored());
                    stack.last_mut().unwrap().insert(file).unwrap();
                }
                next_entry_id += 1;
                updates.set(());
            }
        });
    }
}

impl fs::Tree for Tree {
    fn path(&self) -> &Path {
        &self.path
    }

    fn root(&self) -> &fs::Entry {
        &self.root
    }

    fn updates(&self) -> Box<Stream<Item = (), Error = ()>> {
        Box::new(self.updates.observe())
    }
}
