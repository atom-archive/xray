extern crate xray_core;
#[macro_use]
extern crate criterion;

use criterion::Criterion;
use std::cell::RefCell;
use std::rc::Rc;
use xray_core::buffer::Buffer;
use xray_core::buffer_view::BufferView;

fn bench_edit() {
    let mut editor = BufferView::new(Rc::new(RefCell::new(Buffer::new(1))));
    let content = String::from("abcdefghijklmnopqrstuvwxyz");
    editor.buffer.borrow_mut().splice(0..0, content.as_str());
    for _ in 0..content.len() {
        editor.select_right();
        editor.edit("-");
    }
}

fn edit(c: &mut Criterion) {
    c.bench_function("edit", |b| b.iter(|| bench_edit()));
}

criterion_group!(benches, edit);
criterion_main!(benches);
