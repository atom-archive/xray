#[macro_use]
extern crate napi;

use napi::{Result, Env, Value, Any, Object, futures};

register_module!(test_module, init);

fn init<'env>(env: &'env Env, exports: &'env mut Value<'env, Object>) -> Result<Option<Value<'env, Object>>> {
    exports.set_named_property("testSpawn", env.create_function("testSpawn", callback!(test_spawn)))?;
    Ok(None)
}

fn test_spawn<'a>(env: &'a Env, _this: Value<'a, Any>, _args: &[Value<'a, Any>]) -> Result<Option<Value<'a, Any>>> {
    use std::{thread, time};
    use futures::{Future, Stream};
    use futures::future::Executor;

    let async_context = env.async_init(None, "test_spawn");
    let (promise, deferred) = env.create_promise();
    let (tx, rx) = futures::sync::mpsc::unbounded();

    let future = rx.for_each(|n: usize| {
        println!("Received value {:?}", n);
        futures::future::ok(())
    }).and_then(move |_| {
        async_context.enter(|env| {
            env.resolve_deferred(deferred, env.get_undefined());
        });
        futures::future::ok(())
    });

    env.create_executor().execute(future).unwrap();

    for _i in 0..10 {
        let thread_tx = tx.clone();
        thread::spawn(move || {
            let mut n = 0;
            loop {
                println!("send {:?}", n);
                thread_tx.unbounded_send(n).unwrap();
                n += 1;
                thread::sleep(time::Duration::from_millis(50));
                if n == 10 {
                    break;
                }
            }
        });
    }

    Ok(Some(promise.try_into().unwrap()))
}
