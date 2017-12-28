use std::sync::{Arc, RwLock};
use futures::{Future, Async};

use futures::executor::{self, Notify, Spawn};
use std::mem;
use std::ptr;
use std::os::raw::c_void;
use super::sys;

struct UvHandle(sys::uv_async_t);

struct Task<T: 'static + Future> {
    uv_handle: Arc<RwLock<Option<UvHandle>>>,
    spawn: Spawn<T>
}

struct TaskNotify(Arc<RwLock<Option<UvHandle>>>);

unsafe impl Send for UvHandle {}
unsafe impl Sync for UvHandle {}

impl Notify for TaskNotify {
    fn notify(&self, _id: usize) {
        if let Some(ref uv_handle) = *self.0.read().unwrap() {
            unsafe {
                sys::uv_async_send(mem::transmute(&uv_handle.0));
            }
        }
    }
}

pub fn spawn<T: 'static + Future>(future: T) {
    let spawn = executor::spawn(future);

    unsafe {
        let task = Box::into_raw(Box::new(Task {
            uv_handle: mem::uninitialized(),
            spawn
        }));

        let mut uv_handle: sys::uv_async_t = mem::uninitialized();
        sys::uv_async_init(sys::uv_default_loop(), &mut uv_handle, Some(poll_future_on_main_thread::<T>));
        uv_handle.data = task as *mut c_void;
        ptr::write(&mut (*task).uv_handle, Arc::new(RwLock::new(Some(UvHandle(uv_handle)))));

        // TODO: Fire off an initial poll
    }
}

extern "C" fn poll_future_on_main_thread<T: 'static + Future>(handle: *mut sys::uv_async_t) {
    let mut task: Box<Task<T>> = unsafe { Box::from_raw((*handle).data as *mut Task<T>) };
    let notify = Arc::new(TaskNotify(task.uv_handle.clone()));

    match task.spawn.poll_future_notify(&notify, 0) {
        Ok(Async::Ready(_t)) => {
            // TODO: Drop the task
        },
        Ok(Async::NotReady) => {
            // TODO: Forgot the task
        },
        Err(_e) => {
            // Throw a JS error? Panic?
            panic!("Future yielded an error")
        }
    }

}
