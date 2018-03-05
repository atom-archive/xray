#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
#![allow(dead_code)]
#![cfg_attr(feature = "cargo-clippy", allow(expl_impl_clone_on_copy))]

include!("bindings.rs");

#[cfg(node8)]
mod node8;
#[cfg(node8)]
pub use self::node8::Status;

#[cfg(node9)]
mod node9;
#[cfg(node9)]
pub use self::node9::Status;
