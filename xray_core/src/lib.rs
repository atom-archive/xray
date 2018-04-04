extern crate capnp;
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

pub mod app;
pub mod buffer;
pub mod buffer_view;
pub mod fs;
pub mod notify_cell;
pub mod schema_capnp { include!(concat!(env!("OUT_DIR"), "/src/schema_capnp.rs")); }
pub mod window;
pub mod workspace;

mod file_finder;
mod fuzzy;
mod project;
mod movement;
mod tree;

pub use app::{App, WindowId};
