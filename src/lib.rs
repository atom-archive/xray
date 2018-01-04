#[macro_use]
extern crate napi;
extern crate proton_core;

use std::rc::Rc;
use std::cell::RefCell;
use proton_core::{Buffer, ReplicaId, Editor};
use napi::{sys, Result, Env, Property, Value, Function, Object, futures};

register_module!(proton, init);

fn init<'env>(env: &'env Env, exports: &'env mut Object) -> Result<Option<Object<'env>>> {
    exports.set_named_property("TextBuffer", buffer::init(env))?;
    exports.set_named_property("TextEditor", editor::init(env))?;
    exports.set_named_property("testSpawn", env.create_function("testSpawn", callback!(test_spawn)))?;
    Ok(None)
}

fn test_spawn<'a>(env: &'a Env, this: Value, args: &[Value<'a>]) -> Result<Option<Value<'a>>> {
    use std::{thread, time};
    use futures::{Future, Stream};

    let (tx, rx) = futures::sync::mpsc::unbounded();

    env.spawn(rx.for_each(|n: usize| {
        println!("Received value {:?}", n);
        futures::future::ok(())
    }));

    for _i in 0..10 {
        let thread_tx = tx.clone();
        thread::spawn(move || {
            let mut n = 0;
            loop {
                thread_tx.send(n);
                n += 1;
                thread::sleep(time::Duration::from_millis(50));
                if n == 10 {
                    break;
                }
            }
        });
    }

    Ok(None)
}

mod buffer {
    use super::*;

    pub fn init(env: &Env) -> Function {
        env.define_class("TextBuffer", callback!(constructor), vec![
            Property::new("length").with_getter(callback!(get_length)),
            Property::new("getText").with_method(callback!(get_text)),
            Property::new("splice").with_method(callback!(splice)),
        ])
    }

    fn constructor<'a>(env: &'a Env, mut this: Value, args: &[Value<'a>]) -> Result<Option<Value<'a>>> {
        let replica_id: ReplicaId = args[0].into_number()?.into();
        env.wrap(&mut this, Rc::new(RefCell::new(Buffer::new(replica_id))))?;
        Ok(None)
    }

    fn get_length<'a>(env: &'a Env, this: Value, _args: &[Value]) -> Result<Option<Value<'a>>> {
        let buffer: &Rc<RefCell<Buffer>> = env.unwrap(&this)?;
        Ok(Some(env.create_int64(buffer.borrow().len() as i64).into()))
    }

    fn get_text<'a>(env: &'a Env, this: Value, _args: &[Value]) -> Result<Option<Value<'a>>> {
        let buffer: &Rc<RefCell<Buffer>> = env.unwrap(&this)?;
        Ok(Some(env.create_string_utf16(&buffer.borrow().to_u16_chars()).into()))
    }

    fn splice<'a>(env: &'a Env, this: Value, args: &[Value]) -> Result<Option<Value<'a>>> {
        let start: usize = args[0].into_number()?.into();
        let count: usize = args[1].into_number()?.into();
        let new_text: Vec<u16> = args[2].into_string()?.into();

        let buffer: &Rc<RefCell<Buffer>> = env.unwrap(&this)?;
        buffer.borrow_mut().splice(start..(start + count), new_text);

        Ok(None)
    }
}

mod editor {
    use super::*;

    pub fn init(env: &Env) -> Function {
        env.define_class("TextEditor", callback!(constructor), vec![])
    }

    fn constructor<'a>(env: &'a Env, mut this: Value, args: &[Value<'a>]) -> Result<Option<Value<'a>>> {
        let buffer: &Rc<RefCell<Buffer>> = env.unwrap(&args[0])?;
        let editor = Editor::new(buffer.clone());
        env.wrap(&mut this, editor)?;
        Ok(None)
    }
}
