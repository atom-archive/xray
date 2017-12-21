#[macro_use]
extern crate napi;
extern crate proton_core;

use napi::{sys, Env, Result, Value, Object};

register_module!(proton, init);

fn init<'env>(env: &'env Env, exports: &'env mut Object) -> Result<Option<Object<'env>>> {
    exports.set_named_property("TextBuffer", buffer::init(env))?;
    Ok(None)
}

mod buffer {
    use proton_core::{Buffer, ReplicaId};
    use napi::{Env, Value, Function, Result};

    pub fn init(env: &Env) -> Function {
        env.define_class("TextBuffer", callback!(constructor))
    }

    fn constructor<'a>(env: &'a Env, mut this: Value, args: &[Value<'a>]) -> Result<Option<Value<'a>>> {
       let replica_id: ReplicaId = args[0].into_number()?.into();
       env.wrap(&mut this, Buffer::new(replica_id))?;
       Ok(None)
   }
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
