use std::sync::{Arc, RwLock};
use futures::{Future, Async};

use futures::executor::{self, Notify, Spawn};
use std::mem;
use std::ptr;
use std::os::raw::c_void;
use super::sys;

struct UvHandle(*mut sys::uv_async_t);

struct Task<T: 'static + Future> {
    uv_handle: Arc<RwLock<Option<UvHandle>>>,
    spawn: Spawn<T>
}

struct TaskNotify(Arc<RwLock<Option<UvHandle>>>);

unsafe impl Send for UvHandle {}
unsafe impl Sync for UvHandle {}

impl<T: 'static + Future> Task<T> {
    fn poll_future(&mut self) -> bool {
        let notify = Arc::new(TaskNotify(self.uv_handle.clone()));
        match self.spawn.poll_future_notify(&notify, 0) {
            Ok(Async::Ready(_)) => {
                false
            },
            Ok(Async::NotReady) => {
                true
            },
            Err(_) => panic!("Future yielded an error")
        }
    }
}

impl Notify for TaskNotify {
    fn notify(&self, _id: usize) {
        if let Some(ref uv_handle) = *self.0.read().unwrap() {
            unsafe {
                sys::uv_async_send(uv_handle.0);
            }
        }
    }
}

extern "C" fn poll_future_on_main_thread<T: 'static + Future>(handle: *mut sys::uv_async_t) {
    let mut task: Box<Task<T>> = unsafe { Box::from_raw((*handle).data as *mut Task<T>) };
    if task.poll_future() {
        mem::forget(task); // Don't drop task if it isn't complete.
    }
}

pub fn spawn<T: 'static + Future>(future: T, event_loop: *mut sys::uv_loop_t) {
    let spawn = executor::spawn(future);

    unsafe {
        let task_ptr = Box::into_raw(Box::new(Task {
            uv_handle: mem::uninitialized(),
            spawn
        }));

        let raw_uv_handle: *mut sys::uv_async_t = Box::into_raw(Box::new(mem::uninitialized()));
        let status = sys::uv_async_init(event_loop, raw_uv_handle, Some(poll_future_on_main_thread::<T>));
        assert!(status == 0, "Non-zero status returned from uv_async_init");
        (*raw_uv_handle).data = task_ptr as *mut c_void;
        ptr::write(&mut (*task_ptr).uv_handle, Arc::new(RwLock::new(Some(UvHandle(raw_uv_handle)))));

        if !(*task_ptr).poll_future() {
            Box::from_raw(task_ptr); // Drop task if it is complete.
        }
    }
}
