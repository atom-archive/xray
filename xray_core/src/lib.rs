extern crate bincode;
extern crate bytes;
#[macro_use]
extern crate lazy_static;
extern crate futures;
extern crate parking_lot;
extern crate serde;
#[macro_use]
extern crate serde_derive;
#[macro_use]
extern crate serde_json;
extern crate smallvec;
#[cfg(test)]
extern crate tokio_core;

pub mod app;
pub mod buffer;
pub mod buffer_view;
pub mod fs;
pub mod notify_cell;
pub mod rpc;
pub mod window;
pub mod workspace;

mod file_finder;
mod fuzzy;
mod movement;
mod never;
mod project;
#[cfg(test)]
mod stream_ext;
mod tree;

pub use app::{App, WindowId};
use futures::future::{Executor, Future};
pub use never::Never;
use std::cell::RefCell;
use std::rc::Rc;
pub use window::{ViewId, WindowUpdate};

pub type ForegroundExecutor = Rc<Executor<Box<Future<Item = (), Error = ()> + 'static>>>;
pub type BackgroundExecutor = Rc<Executor<Box<Future<Item = (), Error = ()> + Send + 'static>>>;

pub(crate) trait IntoShared {
    fn into_shared(self) -> Rc<RefCell<Self>>;
}

impl<T> IntoShared for T {
    fn into_shared(self) -> Rc<RefCell<Self>> {
        Rc::new(RefCell::new(self))
    }
}
