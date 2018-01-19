#[macro_use]
extern crate napi;
extern crate xray_core;

use std::rc::Rc;
use std::cell::RefCell;
use xray_core::{Buffer, ReplicaId, Editor};
use napi::{Result, Env, Property, Value, Any, Function, Object, Number, String};

register_module!(xray, init);

fn init<'env>(env: &'env Env, exports: &'env mut Value<'env, Object>) -> Result<Option<Value<'env, Object>>> {
    exports.set_named_property("TextBuffer", buffer::init(env))?;
    exports.set_named_property("TextEditor", editor::init(env))?;
    Ok(None)
}

mod buffer {
    use super::*;

    pub fn init(env: &Env) -> Value<Function> {
        env.define_class("TextBuffer", callback!(constructor), vec![
            Property::new("length").with_getter(callback!(get_length)),
            Property::new("getText").with_method(callback!(get_text)),
            Property::new("splice").with_method(callback!(splice)),
        ])
    }

    fn constructor<'a>(env: &'a Env, mut this: Value<'a, Object>, args: &[Value<'a, Any>]) -> Result<Option<Value<'a, Any>>> {
        let replica_id: ReplicaId = args[0].try_into()?.into();
        env.wrap(&mut this, Rc::new(RefCell::new(Buffer::new(replica_id))))?;
        Ok(None)
    }

    fn get_length<'a>(env: &'a Env, this: Value<'a, Object>, _args: &[Value<'a, Any>]) -> Result<Option<Value<'a, Number>>> {
        let buffer: &Rc<RefCell<Buffer>> = env.unwrap(&this)?;
        Ok(Some(env.create_int64(buffer.borrow().len() as i64)))
    }

    fn get_text<'a>(env: &'a Env, this: Value<'a, Object>, _args: &[Value<'a, Any>]) -> Result<Option<Value<'a, String>>> {
        let buffer: &Rc<RefCell<Buffer>> = env.unwrap(&this)?;
        Ok(Some(env.create_string_utf16(&buffer.borrow().to_u16_chars())))
    }

    fn splice<'a>(env: &'a Env, this: Value<'a, Object>, args: &[Value<'a, Any>]) -> Result<Option<Value<'a, Any>>> {
        let start: usize = args[0].try_into()?.into();
        let count: usize = args[1].try_into()?.into();
        let new_text: Vec<u16> = args[2].try_into()?.into();

        let buffer: &Rc<RefCell<Buffer>> = env.unwrap(&this)?;
        buffer.borrow_mut().splice(start..(start + count), new_text);

        Ok(None)
    }
}

mod editor {
    use super::*;

    pub fn init(env: &Env) -> Value<Function> {
        env.define_class("TextEditor", callback!(constructor), vec![])
    }

    fn constructor<'a>(env: &'a Env, mut this: Value<'a, Object>, args: &[Value<'a, Any>]) -> Result<Option<Value<'a, Any>>> {
        let buffer: &Rc<RefCell<Buffer>> = env.unwrap(&args[0].try_into()?)?;
        let editor = Editor::new(buffer.clone());
        env.wrap(&mut this, editor)?;
        Ok(None)
    }
}
