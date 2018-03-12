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
    use xray_core::buffer::{Buffer, Point};
    use xray_core::editor::{Editor, render};
    use super::*;

    pub fn init(env: &Env) -> Value<Function> {
        env.define_class("TextEditor", callback!(constructor), vec![
            Property::new("addSelection").with_method(callback!(add_selection)),
            Property::new("addSelectionAbove").with_method(callback!(add_selection_above)),
            Property::new("addSelectionBelow").with_method(callback!(add_selection_below)),
            Property::new("moveLeft").with_method(callback!(move_left)),
            Property::new("selectLeft").with_method(callback!(select_left)),
            Property::new("moveRight").with_method(callback!(move_right)),
            Property::new("selectRight").with_method(callback!(select_right)),
            Property::new("moveUp").with_method(callback!(move_up)),
            Property::new("selectUp").with_method(callback!(select_up)),
            Property::new("moveDown").with_method(callback!(move_down)),
            Property::new("selectDown").with_method(callback!(select_down)),
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

    fn add_selection<'a>(env: &'a Env, mut this: Value<'a, Object>, args: &[Value<'a, Any>]) -> Result<Option<Value<'a, Any>>> {
        let editor: &mut Editor = env.unwrap(&mut this)?;
        let start = point_from_js(args[0].try_into::<Object>()?)?;
        let end = point_from_js(args[1].try_into::<Object>()?)?;
        editor.add_selection(start, end);
        Ok(None)
    }

    fn add_selection_above<'a>(env: &'a Env, mut this: Value<'a, Object>, _args: &[Value<'a, Any>]) -> Result<Option<Value<'a, Any>>> {
        let editor: &mut Editor = env.unwrap(&mut this)?;
        editor.add_selection_above();
        Ok(None)
    }

    fn add_selection_below<'a>(env: &'a Env, mut this: Value<'a, Object>, _args: &[Value<'a, Any>]) -> Result<Option<Value<'a, Any>>> {
        let editor: &mut Editor = env.unwrap(&mut this)?;
        editor.add_selection_below();
        Ok(None)
    }

    fn move_left<'a>(env: &'a Env, mut this: Value<'a, Object>, _args: &[Value<'a, Any>]) -> Result<Option<Value<'a, Any>>> {
        let editor: &mut Editor = env.unwrap(&mut this)?;
        editor.move_left();
        Ok(None)
    }

    fn select_left<'a>(env: &'a Env, mut this: Value<'a, Object>, _args: &[Value<'a, Any>]) -> Result<Option<Value<'a, Any>>> {
        let editor: &mut Editor = env.unwrap(&mut this)?;
        editor.select_left();
        Ok(None)
    }

    fn move_right<'a>(env: &'a Env, mut this: Value<'a, Object>, _args: &[Value<'a, Any>]) -> Result<Option<Value<'a, Any>>> {
        let editor: &mut Editor = env.unwrap(&mut this)?;
        editor.move_right();
        Ok(None)
    }

    fn select_right<'a>(env: &'a Env, mut this: Value<'a, Object>, _args: &[Value<'a, Any>]) -> Result<Option<Value<'a, Any>>> {
        let editor: &mut Editor = env.unwrap(&mut this)?;
        editor.select_right();
        Ok(None)
    }

    fn move_up<'a>(env: &'a Env, mut this: Value<'a, Object>, _args: &[Value<'a, Any>]) -> Result<Option<Value<'a, Any>>> {
        let editor: &mut Editor = env.unwrap(&mut this)?;
        editor.move_up();
        Ok(None)
    }

    fn select_up<'a>(env: &'a Env, mut this: Value<'a, Object>, _args: &[Value<'a, Any>]) -> Result<Option<Value<'a, Any>>> {
        let editor: &mut Editor = env.unwrap(&mut this)?;
        editor.select_up();
        Ok(None)
    }

    fn move_down<'a>(env: &'a Env, mut this: Value<'a, Object>, _args: &[Value<'a, Any>]) -> Result<Option<Value<'a, Any>>> {
        let editor: &mut Editor = env.unwrap(&mut this)?;
        editor.move_down();
        Ok(None)
    }

    fn select_down<'a>(env: &'a Env, mut this: Value<'a, Object>, _args: &[Value<'a, Any>]) -> Result<Option<Value<'a, Any>>> {
        let editor: &mut Editor = env.unwrap(&mut this)?;
        editor.select_down();
        Ok(None)
    }

    fn render<'a>(env: &'a Env, this: Value<'a, Object>, args: &[Value<'a, Any>]) -> Result<Option<Value<'a, Any>>> {
        let editor: &Editor = env.unwrap(&this)?;
        let params = args[0].try_into::<Object>()?;

        let frame = editor.render(render::Params {
            scroll_top: params.get_named_property("scrollTop")?.into(),
            height: params.get_named_property("height")?.into(),
            line_height: params.get_named_property("lineHeight")?.into()
        });

        let mut js_frame = env.create_object();

        let mut js_lines = env.create_array_with_length(frame.lines.len());
        for (i, line) in frame.lines.iter().enumerate() {
            js_lines.set_index(i, env.create_string_utf16(line))?;
        }
        js_frame.set_named_property("lines", js_lines)?;

        let mut js_selections = env.create_array_with_length(frame.selections.len());
        for (i, selection) in frame.selections.iter().enumerate() {
            let mut js_selection = env.create_object();
            js_selection.set_named_property("start", point_to_js(env, selection.start)?)?;
            js_selection.set_named_property("end", point_to_js(env, selection.end)?)?;
            js_selection.set_named_property("reversed", env.get_boolean(selection.reversed))?;
            js_selections.set_index(i, js_selection)?;
        }
        js_frame.set_named_property("selections", js_selections)?;

        js_frame.set_named_property("firstVisibleRow", env.create_int64(frame.first_visible_row as i64))?;

        Ok(Some(js_frame.try_into()?))
    }

    fn destroy<'a>(env: &'a Env, this: Value<'a, Object>, _args: &[Value<'a, Any>]) -> Result<Option<Value<'a, Any>>> {
        env.drop_wrapped::<Editor>(this)?;

        Ok(None)
    }

    fn point_to_js(env: &Env, point: Point) -> Result<Value<Object>> {
        let mut js_point = env.create_object();
        js_point.set_named_property("row", env.create_int64(point.row as i64))?;
        js_point.set_named_property("column", env.create_int64(point.column as i64))?;
        Ok(js_point)
    }

    fn point_from_js(js_point: Value<Object>) -> Result<Point> {
        let row: i64 = js_point.get_named_property("row")?.into();
        let column: i64 = js_point.get_named_property("column")?.into();
        Ok(Point::new(row as u32, column as u32))
    }
}
