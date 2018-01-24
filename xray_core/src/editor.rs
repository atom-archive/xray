use std::rc::Rc;
use std::cell::RefCell;
use futures::future::Executor;
use futures::{Future, Stream};
use notify_cell::NotifyCell;

use buffer::{Version, Buffer};

pub struct Editor {
    buffer: Rc<RefCell<Buffer>>,
    version: Rc<NotifyCell<Version>>
}

impl Editor {
    pub fn new(buffer: Rc<RefCell<Buffer>>) -> Self {
        Self {
            buffer,
            version: Rc::new(NotifyCell::new())
        }
    }

    pub fn run<E>(&self, executor: &E)
        where E: Executor<Box<Future<Item = (), Error = ()>>>
    {
        let version_cell = self.version.clone();
        executor.execute(Box::new(self.buffer.borrow().version.observe().for_each(move |buffer_version| {
            version_cell.set(buffer_version);
            Ok(())
        }))).unwrap();
    }
}
