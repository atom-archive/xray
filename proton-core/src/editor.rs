use std::rc::Rc;
use std::cell::RefCell;

use super::buffer::Buffer;

pub struct Editor {
    buffer: Rc<RefCell<Buffer>>
}

impl Editor {
    pub fn new(buffer: Rc<RefCell<Buffer>>) -> Self {
        Self { buffer }
    }
}
