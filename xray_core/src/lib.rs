#![cfg_attr(target_arch = "wasm32", feature(proc_macro, wasm_custom_section, wasm_import_module))]
#![feature(unsize, coerce_unsized)]

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
#[cfg(test)]
extern crate tokio_timer;
#[cfg(target_arch = "wasm32")]
extern crate wasm_bindgen;

#[cfg(target_arch = "wasm32")]
#[macro_use]
pub mod wasm_logging;

pub mod app;
pub mod buffer;
pub mod buffer_view;
pub mod cross_platform;
pub mod fs;
pub mod notify_cell;
pub mod rpc;
pub mod window;
pub mod workspace;

mod discussion;
mod file_finder;
mod fuzzy;
mod movement;
mod project;
#[cfg(test)]
mod stream_ext;
mod tree;

pub use app::{App, WindowId};
use futures::{executor, prelude::*};
use std::cell::RefCell;
use std::rc::Rc;
pub use window::{ViewId, WindowUpdate};

pub trait Executor {
    fn spawn_foreground(
        &self,
        f: Box<Future<Item = (), Error = Never> + 'static>,
    ) -> Result<(), executor::SpawnError>;
    fn spawn_background(
        &self,
        f: Box<Future<Item = (), Error = Never> + Send + 'static>,
    ) -> Result<(), executor::SpawnError>;
}

pub type UserId = usize;

pub(crate) trait IntoShared {
    fn into_shared(self) -> Rc<RefCell<Self>>;
}

impl<T> IntoShared for T {
    fn into_shared(self) -> Rc<RefCell<Self>> {
        Rc::new(RefCell::new(self))
    }
}
