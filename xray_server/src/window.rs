use serde_json;
use std::boxed::Box;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::{Rc, Weak};
use workspace::{WorkspaceHandle, WorkspaceView};
use futures::{Stream, Poll, Async};
use futures::task::{self, Task};

pub type ViewId = usize;

pub trait View {
    fn component_name(&self) -> &'static str;
    fn set_window_handle(&mut self, _handle: WindowHandle) {}
    fn render(&self) -> serde_json::Value;
    fn dispatch_action(&mut self, serde_json::Value);
}

pub struct Window(Rc<RefCell<Inner>>);
pub struct UpdateStream(Weak<RefCell<Inner>>);

pub struct Inner {
    workspace: WorkspaceHandle,
    next_view_id: ViewId,
    views: HashMap<ViewId, Box<View>>,
    inserted: HashSet<ViewId>,
    updated: HashSet<ViewId>,
    removed: HashSet<ViewId>,
    created_update_stream: bool,
    task: Option<Task>,
}

pub struct WindowHandle {
    owner: ViewId,
    inner: Weak<RefCell<Inner>>
}

pub struct ViewHandle {
    owned: ViewId,
    inner: Weak<RefCell<Inner>>
}

#[derive(Serialize, Debug)]
pub struct WindowUpdate {
    updated: Vec<ViewUpdate>,
    removed: Vec<ViewId>
}

#[derive(Serialize, Debug)]
pub struct ViewUpdate {
    component_name: &'static str,
    view_id: ViewId,
    props: serde_json::Value
}

impl Window {
    pub fn new(workspace: WorkspaceHandle) -> Self {
        let window = Window(Rc::new(RefCell::new(Inner {
            workspace: workspace.clone(),
            next_view_id: 0,
            views: HashMap::new(),
            inserted: HashSet::new(),
            updated: HashSet::new(),
            removed: HashSet::new(),
            task: None,
            created_update_stream: false,
        })));
        Inner::add_view(Rc::downgrade(&window.0), Box::new(WorkspaceView::new(workspace)));
        window
    }

    pub fn dispatch_action(&self, view_id: ViewId, action: serde_json::Value) {
        let mut inner = self.0.borrow_mut();
        inner.views.get_mut(&view_id).map(|view| view.dispatch_action(action));
    }

    pub fn updates(&mut self) -> Option<UpdateStream> {
        let mut inner = self.0.borrow_mut();
        if inner.created_update_stream {
            None
        } else {
            inner.created_update_stream = true;
            Some(UpdateStream(Rc::downgrade(&self.0)))
        }
    }
}

impl Stream for UpdateStream {
    type Item = WindowUpdate;
    type Error = ();

    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        let inner = match self.0.upgrade() {
            None => return Ok(Async::Ready(None)),
            Some(inner) => inner
        };

        let mut inner = inner.borrow_mut();

        let mut window_update = WindowUpdate {
            updated: Vec::new(),
            removed: inner.removed.iter().cloned().collect(),
        };

        for id in inner.inserted.iter() {
            if inner.removed.get(&id).is_none() {
                let view = inner.views.get(&id).unwrap();
                window_update.updated.push(ViewUpdate {
                    view_id: *id,
                    component_name: view.component_name(),
                    props: view.render()
                });
            }
        }

        for id in inner.updated.iter() {
            if inner.removed.get(&id).is_none() && inner.inserted.get(&id).is_none() {
                let view = inner.views.get(&id).unwrap();
                window_update.updated.push(ViewUpdate {
                    view_id: *id,
                    component_name: view.component_name(),
                    props: view.render()
                });
            }
        }

        inner.inserted.clear();
        inner.updated.clear();
        inner.removed.clear();

        if window_update.removed.is_empty() && window_update.updated.is_empty() {
            inner.task = Some(task::current());
            Ok(Async::NotReady)
        } else {
            Ok(Async::Ready(Some(window_update)))
        }
    }
}

impl Inner {
    pub fn add_view(inner_ref: Weak<RefCell<Inner>>, mut view: Box<View>) -> ViewId {
        let inner = inner_ref.upgrade().unwrap();
        let mut inner = inner.borrow_mut();
        let view_id = inner.next_view_id;
        inner.next_view_id += 1;
        view.set_window_handle(WindowHandle{owner: view_id, inner: inner_ref});
        inner.views.insert(view_id, view);
        inner.inserted.insert(view_id);
        inner.task.take().map(|task| task.notify());
        view_id
    }
}

impl WindowHandle {
    pub fn add_view(&self, view: Box<View>) -> ViewHandle {
        let view_id = Inner::add_view(self.inner.clone(), view);
        ViewHandle { owned: view_id, inner: self.inner.clone() }
    }

    // TODO - This should probably be `get_view` and return a reference to the `View`,
    // but I couldn't figure out how to get that to work with the borrow checker.
    pub fn render_view(&self, handle: &ViewHandle) -> serde_json::Value {
        let inner = self.inner.upgrade().unwrap();
        let inner = inner.borrow_mut();
        inner.views.get(&handle.owned).unwrap().render()
    }

    pub fn updated(&mut self) {
        let inner = self.inner.upgrade().unwrap();
        let mut inner = inner.borrow_mut();
        inner.updated.insert(self.owner);
        inner.task.take().map(|task| task.notify());
    }
}

impl Drop for ViewHandle {
    fn drop(&mut self) {
        let inner = self.inner.upgrade().unwrap();
        let mut inner = inner.borrow_mut();
        inner.views.remove(&self.owned);
        inner.task.take().map(|task| task.notify());
    }
}
