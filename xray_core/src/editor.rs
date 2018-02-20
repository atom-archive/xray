use std::rc::Rc;
use std::cell::RefCell;
use futures::future::Executor;
use futures::{Future, Stream};
use notify_cell::NotifyCell;

use buffer::{Buffer, Version};

pub struct Editor {
    buffer: Rc<RefCell<Buffer>>,
    pub version: Rc<NotifyCell<Version>>,
    dropped: NotifyCell<bool>,
}

pub struct RenderParams {
    pub scroll_top: f64,
    pub height: f64,
    pub line_height: f64
}

pub struct Frame {
    pub first_visible_row: usize,
    pub lines: Vec<Vec<u16>>
}

impl Editor {
    pub fn new(buffer: Rc<RefCell<Buffer>>) -> Self {
        let version = buffer.borrow().version.get().unwrap();
        Self {
            buffer,
            version: Rc::new(NotifyCell::new(version)),
            dropped: NotifyCell::new(false),
        }
    }

    pub fn run<E>(&self, executor: &E)
    where
        E: Executor<Box<Future<Item = (), Error = ()>>>,
    {
        let version_cell = self.version.clone();
        let buffer_observation = self.buffer.borrow().version.observe().for_each(
            move |buffer_version| {
                version_cell.set(buffer_version);
                Ok(())
            },
        );
        let drop_observation = self.dropped.observe().into_future();
        executor.execute(Box::new(
            buffer_observation
                .select2(drop_observation)
                .then(|_| Ok(())),
        )).unwrap();
    }

    pub fn render(&self, params: RenderParams) -> Frame {
        let buffer = self.buffer.borrow();
        let mut lines = Vec::new();
        let mut cur_line = Vec::new();
        let scroll_bottom = params.scroll_top + params.height;
        let start_row = (params.scroll_top / params.line_height).floor() as usize;
        let end_row = (scroll_bottom / params.line_height).ceil() as usize;

        let mut cur_row = start_row;
        for c in buffer.iter_starting_at_row(start_row) {
            if c == (b'\n' as u16) {
                lines.push(cur_line);
                cur_line = Vec::new();
                cur_row += 1;
                if cur_row >= end_row {
                    break;
                }
            } else {
                cur_line.push(c);
            }
        }
        if cur_row < end_row {
            lines.push(cur_line);
        }

        Frame {
            first_visible_row: start_row,
            lines
        }
    }
}

impl Drop for Editor {
    fn drop(&mut self) {
        self.dropped.set(true);
    }
}

#[cfg(test)]
mod tests {
    extern crate tokio_core;

    use super::*;
    use self::tokio_core::reactor::Core;
    use futures::future;

    #[test]
    fn test_version_updates() {
        let mut event_loop = Core::new().unwrap();
        let buffer = Rc::new(RefCell::new(Buffer::new(1)));
        let editor = Editor::new(buffer.clone());
        editor.run(&event_loop);
        buffer.borrow_mut().splice(0..0, "test");
        event_loop.run(editor.version.observe().take(1).into_future());
    }
}
