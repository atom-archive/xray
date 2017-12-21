#[macro_use]
extern crate napi;
extern crate proton_core;

use napi::{sys, Result, Env, Value, Object};

register_module!(proton, init);

fn init<'env>(env: &'env Env, exports: &'env mut Object) -> Result<Option<Object<'env>>> {
    exports.set_named_property("TextBuffer", buffer::init(env))?;
    Ok(None)
}

mod buffer {
    use proton_core::{Buffer, ReplicaId};
    use napi::{Env, Property, Value, Function, Result};

    pub fn init(env: &Env) -> Function {
        env.define_class("TextBuffer", callback!(constructor), vec![
            Property::new("length").with_getter(callback!(get_length))
        ])
    }

    fn constructor<'a>(env: &'a Env, mut this: Value, args: &[Value<'a>]) -> Result<Option<Value<'a>>> {
        let replica_id: ReplicaId = args[0].into_number()?.into();
        env.wrap(&mut this, Buffer::new(replica_id))?;
        Ok(None)
    }

    fn get_length<'a>(env: &'a Env, this: Value, _args: &[Value]) -> Result<Option<Value<'a>>> {
       let buffer: &Buffer = unsafe { env.unwrap(&this)? };
       Ok(Some(env.create_int64(buffer.len() as i64).into()))
    }
}
