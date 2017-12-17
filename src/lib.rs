extern crate napi;
extern crate proton_core;

use napi::{sys, Env, Result, Value, Object};
// use proton_core::{Buffer, ReplicaId};
use std::io::Write;
use std::ptr;

#[no_mangle] pub extern "C" fn init_module_from_c(env: sys::napi_env, exports: sys::napi_value) -> sys::napi_value {
    let env = Env::from(env);

    let mut exports = Value::from_raw(&env, exports).into_object()
        .expect("Expected an exports object to be passed to module initializer");

    match init_module(&env, &mut exports) {
        Ok(Some(exports)) => exports.into(),
        Ok(None) => ptr::null_mut(),
        Err(e) => {
            let _ = writeln!(std::io::stderr(), "Error initializing module: {:?}", e);
            ptr::null_mut()
        }
    }
}

fn init_module<'env>(env: &'env Env, exports: &'env mut Object) -> Result<Option<Object<'env>>> {
    exports.set_named_property("foo", env.create_int64(42))?;
    Ok(None)
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
