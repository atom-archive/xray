use BackgroundExecutor;
use serde_json;
use std::boxed::Box;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::{Rc, Weak};
use futures::{Async, Future, Poll, Stream};
use futures::task::{self, Task};

pub type ViewId = usize;

pub trait View: Stream<Item = (), Error = ()> {
    fn component_name(&self) -> &'static str;
    fn will_mount(&mut self, &mut Window, WeakViewHandle<Self>) where Self: Sized {}
    fn render(&self) -> serde_json::Value;
    fn dispatch_action(&mut self, serde_json::Value, &mut Window) {}
}

pub struct Window(Rc<RefCell<Inner>>, Option<ViewHandle>);
pub struct WindowUpdateStream {
    counter: usize,
    polled_once: bool,
    inner: Weak<RefCell<Inner>>,
}

pub struct Inner {
    background: Option<BackgroundExecutor>,
    next_view_id: ViewId,
    views: HashMap<ViewId, Rc<RefCell<View<Item = (), Error = ()>>>>,
    inserted: HashSet<ViewId>,
    removed: HashSet<ViewId>,
    focused: Option<ViewId>,
    height: f64,
    update_stream_counter: usize,
    update_stream_task: Option<Task>,
}

pub struct ViewHandle {
    pub view_id: ViewId,
    inner: Weak<RefCell<Inner>>,
}

pub struct WeakViewHandle<T>(Weak<RefCell<T>>);

#[derive(Serialize, Debug)]
pub struct WindowUpdate {
    updated: Vec<ViewUpdate>,
    removed: Vec<ViewId>,
    focused: Option<ViewId>
}

#[derive(Serialize, Debug)]
pub struct ViewUpdate {
    component_name: &'static str,
    view_id: ViewId,
    props: serde_json::Value,
}

impl Window {
    pub fn new(background: Option<BackgroundExecutor>, height: f64) -> Self {
        Window(
            Rc::new(RefCell::new(Inner {
                background,
                next_view_id: 0,
                views: HashMap::new(),
                inserted: HashSet::new(),
                removed: HashSet::new(),
                focused: None,
                height: height,
                update_stream_counter: 0,
                update_stream_task: None,
            })),
            None,
        )
    }

    pub fn dispatch_action(&mut self, view_id: ViewId, action: serde_json::Value) {
        let view = self.0.borrow().get_view(view_id);
        view.map(|view| view.borrow_mut().dispatch_action(action, self));
    }

    pub fn updates(&mut self) -> WindowUpdateStream {
        let mut inner = self.0.borrow_mut();
        inner.update_stream_counter += 1;
        WindowUpdateStream {
            counter: inner.update_stream_counter,
            polled_once: false,
            inner: Rc::downgrade(&self.0),
        }
    }

    pub fn set_height(&mut self, height: f64) {
        self.0.borrow_mut().height = height;
    }

    pub fn set_root_view(&mut self, root_view: ViewHandle) {
        self.1 = Some(root_view);
    }

    pub fn height(&self) -> f64 {
        self.0.borrow().height
    }

    pub fn add_view<T: 'static + View>(&mut self, view: T) -> ViewHandle {
        let view_id = {
            let mut inner = self.0.borrow_mut();
            inner.next_view_id += 1;
            inner.next_view_id - 1
        };

        let view_rc = Rc::new(RefCell::new(view));
        let weak_view = Rc::downgrade(&view_rc);
        view_rc.borrow_mut().will_mount(self, WeakViewHandle(weak_view));

        let mut inner = self.0.borrow_mut();
        inner.views.insert(view_id, view_rc);
        inner.inserted.insert(view_id);
        inner.notify();
        ViewHandle {
            view_id,
            inner: Rc::downgrade(&self.0),
        }
    }

    pub fn spawn<F: Future<Item = (), Error = ()> + Send + 'static>(&self, future: F) {
        self.0.borrow().background.as_ref().map(|background| background.execute(Box::new(future)));
    }
}

impl Stream for WindowUpdateStream {
    type Item = WindowUpdate;
    type Error = ();

    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        let inner_ref = match self.inner.upgrade() {
            None => return Ok(Async::Ready(None)),
            Some(inner) => inner,
        };

        let mut window_update;
        {
            let mut inner = inner_ref.borrow_mut();

            if self.counter < inner.update_stream_counter {
                return Ok(Async::Ready(None));
            }

            if self.polled_once {
                window_update = WindowUpdate {
                    updated: Vec::new(),
                    removed: inner.removed.iter().cloned().collect(),
                    focused: inner.focused.take()
                };

                for id in inner.inserted.iter() {
                    if !inner.removed.contains(&id) {
                        let view = inner.get_view(*id).unwrap();
                        let view = view.borrow();
                        window_update.updated.push(ViewUpdate {
                            view_id: *id,
                            component_name: view.component_name(),
                            props: view.render(),
                        });
                    }
                }

                for (id, ref view) in inner.views.iter() {
                    let result = view.borrow_mut().poll();
                    if !inner.inserted.contains(&id) {
                        if let Ok(Async::Ready(Some(()))) = result {
                            let view = view.borrow();
                            window_update.updated.push(ViewUpdate {
                                view_id: *id,
                                component_name: view.component_name(),
                                props: view.render(),
                            });
                        }
                    }
                }
            } else {
                window_update = WindowUpdate {
                    updated: Vec::new(),
                    removed: Vec::new(),
                    focused: inner.focused.take()
                };

                for (id, ref view) in inner.views.iter() {
                    let mut view = view.borrow_mut();
                    let _ = view.poll();
                    window_update.updated.push(ViewUpdate {
                        view_id: *id,
                        component_name: view.component_name(),
                        props: view.render(),
                    });
                }

                self.polled_once = true;
            }
        }

        let mut inner = inner_ref.borrow_mut();
        inner.inserted.clear();
        inner.removed.clear();

        if window_update.removed.is_empty() && window_update.updated.is_empty() {
            inner.update_stream_task = Some(task::current());
            Ok(Async::NotReady)
        } else {
            Ok(Async::Ready(Some(window_update)))
        }
    }
}

impl Inner {
    fn notify(&mut self) {
        self.update_stream_task.take().map(|task| task.notify());
    }

    fn get_view(&self, id: ViewId) -> Option<Rc<RefCell<View<Item = (), Error = ()>>>> {
        self.views.get(&id).map(|view| view.clone())
    }
}

impl ViewHandle {
    pub fn focus(&self) -> Result<(), ()> {
        let inner = self.inner.upgrade().ok_or(())?;
        let mut inner = inner.borrow_mut();
        inner.focused = Some(self.view_id);
        inner.notify();
        Ok(())
    }
}

impl Drop for ViewHandle {
    fn drop(&mut self) {
        // Store the removed view here to prevent it from being dropped until after the borrow of
        // inner is dropped, since the removed view may itself hold other view handles which will
        // call drop reentrantly.
        let mut _removed_view = None;

        let inner = self.inner.upgrade();
        if let Some(inner) = inner {
            let mut inner = inner.borrow_mut();
            _removed_view = inner.views.remove(&self.view_id);
            inner.removed.insert(self.view_id);
            inner.notify();
        }
    }
}

impl<T> WeakViewHandle<T> {
    pub fn map<F, R>(&self, f: F) -> Option<R>
    where
        F: FnOnce(&mut T) -> R
    {
        self.0.upgrade().map(|view| f(&mut *view.borrow_mut()))
    }
}

impl<T> Clone for WeakViewHandle<T> {
    fn clone(&self) -> Self {
        WeakViewHandle(self.0.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_view_handle_drop() {
        // Dropping the window should not cause a panic
        let mut window = Window::new(None, 100.0);
        window.add_view(TestView::new(true));
    }

    struct TestView {
        add_child: bool,
        handle: Option<ViewHandle>,
        updates: NotifyCell<()>,
    }

    use notify_cell::NotifyCell;

    impl TestView {
        fn new(add_child: bool) -> Self {
            TestView {
                add_child,
                handle: None,
                updates: NotifyCell::new(()),
            }
        }
    }

    impl View for TestView {
        fn component_name(&self) -> &'static str {
            "TestView"
        }

        fn render(&self) -> serde_json::Value {
            json!({})
        }

        fn will_mount(&mut self, window: &mut Window, _view_handle: WeakViewHandle<Self>) {
            if self.add_child {
                self.handle = Some(window.add_view(TestView::new(false)));
            }
        }
    }

    impl Stream for TestView {
        type Item = ();
        type Error = ();

        fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
            self.updates.poll()
        }
    }
}
