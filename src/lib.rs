extern crate napi;
extern crate proton_core;

use napi::{sys, Env, Result, Value, Object};
// use proton_core::{Buffer, ReplicaId};
use std::io::Write;
use std::ptr;

#[no_mangle] pub extern "C" fn init_module(raw_env: sys::napi_env, raw_exports: sys::napi_value) -> sys::napi_value {
    napi::init_module(raw_env, raw_exports, |env, exports| {
        exports.set_named_property("foo", env.create_int64(42))?;
        Ok(None)
    })
}

// let env = napi::Env::from(env);
//
// let text_buffer_constructor = env.define_class(
//     "TextBuffer",
//     env.create_function("TextBuffer", 1, |this: napi::Value, args: Vec<napi::Value>| {
//         let replica_id = args[0].try_into::<napi::Number>().unwrap().value_int64() as ReplicaId;
//         env.wrap_object(this, Buffer::new(replica_id)).unwrap();
//     }).unwrap(),
//     [
//         napi::PropertyDescriptor::new("length").with_getter(|this: napi::Value| {
//             let buffer = env.unwrap_object().unwrap();
//             env.create_int64(buffer.len())
//         })
//     ]
// );
//
// let exports = napi::Object::try_from(&env, exports).unwrap();
// exports.set_named_property("TextBuffer", text_buffer_constructor).unwrap();
// exports.into()
