#[macro_use]
extern crate napi;
extern crate xray_core;

use std::rc::Rc;
use std::cell::RefCell;
use napi::{futures, Result, Env, Property, Value, Ref, Any, Function, Object, Number, String};
use futures::future::Executor;
use futures::Stream;

register_module!(xray, init);

fn init<'env>(env: &'env Env, exports: &'env mut Value<'env, Object>) -> Result<Option<Value<'env, Object>>> {
    exports.set_named_property("TextBuffer", buffer::init(env))?;
    exports.set_named_property("TextEditor", editor::init(env))?;
    Ok(None)
}

mod buffer {
    use super::*;
    use xray_core::buffer::{Buffer, ReplicaId};

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
    use xray_core::buffer::Buffer;
    use xray_core::editor::{Editor, RenderParams};
    use super::*;

    pub fn init(env: &Env) -> Value<Function> {
        env.define_class("TextEditor", callback!(constructor), vec![
            Property::new("render").with_method(callback!(render)),
            Property::new("destroy").with_method(callback!(destroy))
        ])
    }

    fn constructor<'a>(env: &'a Env, mut this: Value<'a, Object>, args: &[Value<'a, Any>]) -> Result<Option<Value<'a, Any>>> {
        let executor = env.create_executor();

        let buffer: &Rc<RefCell<Buffer>> = env.unwrap(&args[0].try_into()?)?;
        let editor = Editor::new(buffer.clone());
        editor.run(&executor);

        let on_change_cb: Ref<Function> = env.create_reference(&args[1].try_into()?);
        let async_context = env.async_init(None, "editor.onChange");
        executor.execute(editor.version.observe().for_each(move |_| {
            async_context.enter(|&mut env| {
                let on_change_cb = env.get_reference_value(&on_change_cb);
                on_change_cb.call(None, &[]).unwrap();
            });

            Ok(())
        })).unwrap();

        env.wrap(&mut this, editor)?;
        Ok(None)
    }

    fn render<'a>(env: &'a Env, this: Value<'a, Object>, args: &[Value<'a, Any>]) -> Result<Option<Value<'a, Any>>> {
        let editor: &Editor = env.unwrap(&this)?;
        let params = args[0].try_into::<Object>()?;

        let frame = editor.render(RenderParams {
            scroll_top: params.get_named_property("scrollTop")?.into(),
            offset_height: params.get_named_property("offsetHeight")?.into(),
            line_height: params.get_named_property("lineHeight")?.into()
        });

        let mut js_frame = env.create_object();
        let mut js_lines = env.create_array_with_length(frame.lines.len());

        for (i, line) in frame.lines.iter().enumerate() {
            js_lines.set_index(i, env.create_string_utf16(line))?;
        }
        js_frame.set_named_property("lines", js_lines)?;

        Ok(Some(js_frame.try_into()?))
    }

    fn destroy<'a>(env: &'a Env, this: Value<'a, Object>, _args: &[Value<'a, Any>]) -> Result<Option<Value<'a, Any>>> {
        env.drop_wrapped::<Editor>(this)?;

        Ok(None)
    }
}
