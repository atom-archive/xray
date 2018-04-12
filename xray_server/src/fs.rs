use futures::{self, future, Future, Stream};
use ignore::WalkBuilder;
use std::ffi::OsString;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use xray_core::fs;
use xray_core::notify_cell::NotifyCell;
use xray_core::BackgroundExecutor;

pub struct Tree {
    path: PathBuf,
    root: fs::Entry,
    updates: Arc<NotifyCell<()>>,
}

pub struct IoProvider {
    background: BackgroundExecutor,
}

impl Tree {
    pub fn new<T: Into<PathBuf>>(path: T) -> Result<Self, &'static str> {
        let path = path.into();
        let file_name = OsString::from(path.file_name().ok_or("Path must have a filename")?);
        let root = fs::Entry::dir(file_name, false, false);
        let updates = Arc::new(NotifyCell::new(()));
        Self::populate(path.clone(), root.clone(), updates.clone());
        Ok(Self {
            path,
            root,
            updates,
        })
    }

    fn populate(path: PathBuf, root: fs::Entry, updates: Arc<NotifyCell<()>>) {
        thread::spawn(move || {
            let mut stack = vec![root];

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
                    let dir = fs::Entry::dir(
                        OsString::from(file_name),
                        file_type.is_symlink(),
                        entry.ignored(),
                    );
                    stack.last_mut().unwrap().insert(dir.clone()).unwrap();
                    stack.push(dir);
                } else if file_type.is_file() {
                    let file = fs::Entry::file(
                        OsString::from(file_name),
                        file_type.is_symlink(),
                        entry.ignored(),
                    );
                    stack.last_mut().unwrap().insert(file).unwrap();
                }
                updates.set(());
            }
        });
    }
}

impl fs::Tree for Tree {
    fn root(&self) -> fs::Entry {
        self.root.clone()
    }

    fn updates(&self) -> Box<Stream<Item = (), Error = ()>> {
        Box::new(self.updates.observe())
    }
}

impl fs::LocalTree for Tree {
    fn path(&self) -> &Path {
        &self.path
    }

    fn populated(&self) -> Box<Future<Item = (), Error = ()>> {
        unimplemented!()
    }

    fn as_tree(&self) -> &fs::Tree {
        self
    }
}

impl IoProvider {
    pub fn new(background: BackgroundExecutor) -> Self {
        IoProvider { background }
    }
}

impl fs::IoProvider for IoProvider {
    fn read(&self, path: &Path) -> Box<Future<Item = String, Error = io::Error>> {
        let path = path.to_owned();

        let (tx, rx) = futures::sync::oneshot::channel();

        self.background.execute(Box::new(future::lazy(move || {
            fn read(path: PathBuf) -> Result<String, io::Error> {
                let file = File::open(path)?;
                let mut buf_reader = io::BufReader::new(file);
                let mut contents = String::new();
                buf_reader.read_to_string(&mut contents)?;
                Ok(contents)
            }

            let _ = tx.send(read(path));
            Ok(())
        }))).unwrap();

        Box::new(rx.then(|result|
            result.unwrap_or(
                Err(io::Error::new(io::ErrorKind::Interrupted, "The read task was dropped"))
            )
        ))
    }
}
