use serde_json;
use std::boxed::Box;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use workspace::{WorkspaceHandle, WorkspaceView};

pub type ViewId = usize;

pub trait View {
    fn component_name(&self) -> &'static str;
    fn set_window_handle(&mut self, _handle: WindowHandle) {}
    fn render(&self) -> serde_json::Value;
    fn dispatch_action(&mut self, serde_json::Value);
    // fn changes(&self) -> NotifyCell<()>;
}

pub struct Window(Rc<RefCell<Inner>>);

pub struct WindowHandle(Weak<RefCell<Inner>>);

pub struct Inner {
    workspace: WorkspaceHandle,
    next_view_id: ViewId,
    views: HashMap<ViewId, Box<View>>,
}

pub struct ViewHandle {
    view_id: ViewId,
    window_handle: WindowHandle
}

impl Window {
    pub fn new(workspace: WorkspaceHandle) -> Self {
        let window = Window(Rc::new(RefCell::new(Inner {
            workspace: workspace.clone(),
            next_view_id: 0,
            views: HashMap::new()
        })));
        window.handle().add_view(Box::new(WorkspaceView::new(workspace)));
        window
    }

    pub fn dispatch_action(&self, view_id: ViewId, action: serde_json::Value) {
        let mut inner = self.0.borrow_mut();
        inner.views.get_mut(&view_id).map(|view| view.dispatch_action(action));
    }

    fn handle(&self) -> WindowHandle {
        WindowHandle(Rc::downgrade(&self.0))
    }
}

impl WindowHandle {
    pub fn add_view(&self, mut view: Box<View>) -> ViewHandle {
        let inner = self.0.upgrade().unwrap();
        let mut inner = inner.borrow_mut();
        let view_id = inner.next_view_id;
        inner.next_view_id += 1;
        view.set_window_handle(WindowHandle(self.0.clone()));
        inner.views.insert(view_id, view);

        ViewHandle {
            view_id,
            window_handle: WindowHandle(self.0.clone())
        }
    }
}

impl Drop for ViewHandle {
    fn drop(&mut self) {
        let window_inner = self.window_handle.0.upgrade().unwrap();
        window_inner.borrow_mut().views.remove(&self.view_id);
    }
}
