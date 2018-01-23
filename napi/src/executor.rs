use std::sync::{Arc, RwLock};
use futures::{Future, Async};
use futures::future::{Executor, ExecuteError};
use futures::executor::{self, Notify, Spawn};
use std::mem;
use std::ptr;
use std::os::raw::c_void;
use super::sys;

pub struct LibuvExecutor {
    event_loop: *mut sys::uv_loop_t
}

struct Task<T: 'static + Future> {
    spawn: Spawn<T>,
    notify_handle: Arc<TaskNotifyHandle>
}

struct TaskNotifyHandle(RwLock<Option<UvAsyncHandle>>);

struct UvAsyncHandle(Box<sys::uv_async_t>);

impl LibuvExecutor {
    pub fn new(event_loop: *mut sys::uv_loop_t) -> Self {
        Self { event_loop }
    }
}

impl<F> Executor<F> for LibuvExecutor
    where F: 'static + Future<Item = (), Error = ()>
{

    fn execute(&self, future: F) -> Result<(), ExecuteError<F>> {
        let spawn = executor::spawn(future);

        unsafe {
            let mut task = Box::new(Task {
                spawn,
                notify_handle: mem::uninitialized()
            });

            ptr::write(&mut task.notify_handle, Arc::new(TaskNotifyHandle::new(
                self.event_loop,
                Some(poll_future_on_main_thread::<F>),
                mem::transmute_copy(&task)
            )));

            if !task.poll_future() {
                mem::forget(task)
            }
        }

        Ok(())
    }
}

impl<T: 'static + Future> Task<T> {
    fn poll_future(&mut self) -> bool {
        match self.spawn.poll_future_notify(&self.notify_handle, 0) {
            Ok(Async::Ready(_)) => {
                let mut handle = self.notify_handle.0.write().unwrap().take().unwrap();
                handle.close();
                true
            },
            Ok(Async::NotReady) => false,
            Err(_) => panic!("Future yielded an error")
        }
    }
}

impl TaskNotifyHandle {
    fn new(event_loop: *mut sys::uv_loop_t, callback: sys::uv_async_cb, data: *mut c_void) -> Self {
        TaskNotifyHandle(RwLock::new(Some(UvAsyncHandle::new(event_loop, callback, data))))
    }
}

impl Notify for TaskNotifyHandle {
    fn notify(&self, _id: usize) {
        if let Some(ref uv_handle) = *self.0.read().unwrap() {
            unsafe {
                sys::uv_async_send(mem::transmute_copy(&uv_handle.0));
            }
        }
    }
}

impl UvAsyncHandle {
    fn new(event_loop: *mut sys::uv_loop_t, callback: sys::uv_async_cb, data: *mut c_void) -> Self {
        unsafe {
            let mut handle = UvAsyncHandle(Box::new(mem::uninitialized()));
            let status = sys::uv_async_init(event_loop, mem::transmute_copy(&handle.0), callback);
            assert!(status == 0, "Non-zero status returned from uv_async_init");
            handle.0.data = data;
            handle
        }
    }

    fn close(self) {
        unsafe {
            sys::uv_close(mem::transmute_copy(&self.0), Some(drop_handle_after_close));
            mem::forget(self.0);
        }
    }
}

unsafe impl Send for UvAsyncHandle {}
unsafe impl Sync for UvAsyncHandle {}

extern "C" fn drop_handle_after_close(handle: *mut sys::uv_handle_t) {
    unsafe {
        Box::from_raw(handle);
    }
}

extern "C" fn poll_future_on_main_thread<T: 'static + Future>(handle: *mut sys::uv_async_t) {
    let mut task: Box<Task<T>> = unsafe { Box::from_raw((*handle).data as *mut Task<T>) };
    if !task.poll_future() {
        mem::forget(task); // Don't drop task if it isn't complete.
    }
}
