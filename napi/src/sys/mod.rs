#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
#![allow(dead_code)]
#![cfg_attr(feature = "cargo-clippy", allow(expl_impl_clone_on_copy))]

include!("bindings.rs");

#[cfg(feature = "node8")]
mod node8;
#[cfg(feature = "node8")]
pub use self::node8::Status;

#[cfg(feature = "node9")]
mod node9;
#[cfg(feature = "node9")]
pub use self::node9::Status;
