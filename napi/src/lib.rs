pub extern crate futures;
use std::any::TypeId;
use std::ffi::CString;
use std::os::raw::{c_char, c_void};
use std::marker::PhantomData;
use std::mem;
use std::ptr;
use std::string::String as RustString;
use futures::Future;

pub mod sys;
mod async;

pub type Result<T> = std::result::Result<T, Error>;
pub type Callback = extern "C" fn(sys::napi_env, sys::napi_callback_info) -> sys::napi_value;

#[derive(Debug)]
pub struct Error {
    status: Status
}

#[derive(Eq, PartialEq, Debug)]
pub enum Status {
    Ok,
    InvalidArg,
    ObjectExpected,
    StringExpected,
    NameExpected,
    FunctionExpected,
    NumberExpected,
    BooleanExpected,
    ArrayExpected,
    GenericFailure,
    PendingException,
    Cancelled,
    EscapeCalledTwice,
    HandleScopeMismatch,
    StringContainsNull
}

#[derive(Clone, Copy, Debug)]
pub struct Env(sys::napi_env);

// Value types
#[derive(Clone, Copy, Debug)]
pub struct Any;

#[derive(Clone, Copy, Debug)]
pub struct Undefined;

#[derive(Clone, Copy, Debug)]
pub struct Number;

#[derive(Clone, Copy, Debug)]
pub struct String;

#[derive(Clone, Copy, Debug)]
pub struct Object;

#[derive(Clone, Copy, Debug)]
pub struct Function;

#[derive(Clone, Copy, Debug)]
pub struct Value<'env, T> {
    env: &'env Env,
    raw_value: sys::napi_value,
    _marker: PhantomData<T>
}

pub struct AsyncContext {
    raw_env: sys::napi_env,
    raw_context: sys::napi_async_context,
    raw_resource: sys::napi_ref
}

pub struct Deferred(sys::napi_deferred);

#[derive(Clone, Debug)]
pub struct Property {
    name: RustString,
    raw_descriptor: sys::napi_property_descriptor
}

#[repr(C)]
struct TaggedObject<T> {
    type_id: TypeId,
    object: T,
}

#[macro_export]
macro_rules! register_module {
    ($module_name:ident, $init:ident) => {
        #[no_mangle]
        #[cfg_attr(target_os = "linux", link_section = ".ctors")]
        #[cfg_attr(target_os = "macos", link_section = "__DATA,__mod_init_func")]
        #[cfg_attr(target_os = "windows", link_section = ".CRT$XCU")]
        pub static __REGISTER_MODULE: extern "C" fn() = {
            use $crate::sys;
            use ::std::io::Write;
            use ::std::os::raw::c_char;
            use ::std::ptr;

            extern "C" fn register_module() {
                static mut MODULE_DESCRIPTOR: Option<sys::napi_module> = None;
                unsafe {
                    MODULE_DESCRIPTOR = Some(sys::napi_module {
                        nm_version: 1,
                        nm_flags: 0,
                        nm_filename: concat!(file!(), "\0").as_ptr() as *const c_char,
                        nm_register_func: Some(init_module),
                        nm_modname: concat!(stringify!($module_name), "\0").as_ptr() as *const c_char,
                        nm_priv: 0 as *mut _,
                        reserved: [0 as *mut _; 4]
                    });

                    sys::napi_module_register(MODULE_DESCRIPTOR.as_mut().unwrap() as *mut sys::napi_module);
                }

                extern "C" fn init_module(raw_env: sys::napi_env, raw_exports: sys::napi_value) -> sys::napi_value {
                    let env = Env::from_raw(raw_env);
                    let mut exports: Value<Object> = Value::from_raw(&env, raw_exports);

                    let result = $init(&env, &mut exports);

                    match result {
                        Ok(Some(exports)) => exports.into_raw(),
                        Ok(None) => ptr::null_mut(),
                        Err(e) => {
                            let _ = writeln!(::std::io::stderr(), "Error initializing module: {:?}", e);
                            ptr::null_mut()
                        }
                    }
                }
            }

            register_module
        };
    }
}

#[macro_export]
macro_rules! callback {
    ($callback_expr:expr) => {
        {
            use std::io::Write;
            use ::std::mem;
            use ::std::os::raw::c_char;
            use ::std::ptr;
            use $crate::sys;
            use $crate::{Env, Status, Value, Any};

            extern "C" fn raw_callback(raw_env: sys::napi_env, cb_info: sys::napi_callback_info) -> sys::napi_value {
                const MAX_ARGC: usize = 8;
                let mut argc = MAX_ARGC;
                let mut raw_args: [$crate::sys::napi_value; MAX_ARGC] = unsafe { mem::uninitialized() };
                let mut raw_this = ptr::null_mut();

                unsafe {
                    let status = sys::napi_get_cb_info(
                        raw_env,
                        cb_info,
                        &mut argc,
                        &mut raw_args[0],
                        &mut raw_this,
                        ptr::null_mut()
                    );
                    debug_assert!(Status::from(status) == Status::Ok);
                }

                let env = Env::from_raw(raw_env);
                let this = Value::from_raw(&env, raw_this);
                let mut args: [Value<Any>; 8] = unsafe { mem::uninitialized() };
                for (i, raw_arg) in raw_args.into_iter().enumerate() {
                    args[i] = Value::from_raw(&env, *raw_arg)
                }

                let callback = $callback_expr;
                let result = callback(&env, this, &args[0..argc]);

                match result {
                    Ok(Some(result)) => result.into_raw(),
                    Ok(None) => env.get_undefined().into_raw(),
                    Err(e) => {
                        let _ = writeln!(::std::io::stderr(), "Error calling function: {:?}", e);
                        let message = format!("{:?}", e);
                        unsafe {
                            $crate::sys::napi_throw_error(
                                raw_env,
                                ptr::null(),
                                message.as_ptr() as *const c_char
                            );
                        }
                        env.get_undefined().into_raw()
                    }
                }
            }

            raw_callback
        }
    }
}

impl From<std::ffi::NulError> for Error {
    fn from(_error: std::ffi::NulError) -> Self {
        Error { status: Status::StringContainsNull }
    }
}

impl From<sys::napi_status> for Status {
    fn from(code: sys::napi_status) -> Self {
        use sys::napi_status::*;
        use Status::*;

        match code {
            napi_ok => Ok,
            napi_invalid_arg => InvalidArg,
            napi_object_expected => ObjectExpected,
            napi_string_expected => StringExpected,
            napi_name_expected => NameExpected,
            napi_function_expected => FunctionExpected,
            napi_number_expected => NumberExpected,
            napi_boolean_expected => BooleanExpected,
            napi_array_expected => ArrayExpected,
            napi_generic_failure => GenericFailure,
            napi_pending_exception => PendingException,
            napi_cancelled => Cancelled,
            napi_escape_called_twice => EscapeCalledTwice,
            napi_handle_scope_mismatch => HandleScopeMismatch
        }
    }
}

impl Env {
    pub fn from_raw(env: sys::napi_env) -> Self {
        Env(env)
    }

    pub fn get_undefined<'a>(&'a self) -> Value<'a, Undefined> {
        let mut raw_value = ptr::null_mut();
        let status = unsafe {
            sys::napi_get_undefined(self.0, &mut raw_value)
        };
        debug_assert!(Status::from(status) == Status::Ok);
        Value::from_raw(self, raw_value)
    }

    pub fn create_int64<'a>(&'a self, int: i64) -> Value<'a, Number> {
        let mut raw_value = ptr::null_mut();
        let status = unsafe {
            sys::napi_create_int64(self.0, int, (&mut raw_value) as *mut sys::napi_value)
        };
        debug_assert!(Status::from(status) == Status::Ok);
        Value::from_raw(self, raw_value)
    }

    pub fn create_string<'a, 'b>(&'a self, s: &'b str) -> Value<'a, String> {
        let mut raw_value = ptr::null_mut();
        let status = unsafe {
            sys::napi_create_string_utf8(self.0, s.as_ptr() as *const c_char, s.len(), &mut raw_value)
        };
        debug_assert!(Status::from(status) == Status::Ok);
        Value::from_raw(self, raw_value)
    }

    pub fn create_string_utf16<'a, 'b>(&'a self, chars: &[u16]) -> Value<'a, String> {
        let mut raw_value = ptr::null_mut();
        let status = unsafe {
            sys::napi_create_string_utf16(self.0, chars.as_ptr(), chars.len(), &mut raw_value)
        };
        debug_assert!(Status::from(status) == Status::Ok);
        Value::from_raw(self, raw_value)
    }

    pub fn create_object<'a>(&'a self) -> Value<'a, Object> {
        let mut raw_value = ptr::null_mut();
        let status = unsafe {
            sys::napi_create_object(self.0, &mut raw_value)
        };
        debug_assert!(Status::from(status) == Status::Ok);
        Value::from_raw(self, raw_value)
    }

    pub fn create_function<'a, 'b>(&'a self, name: &'b str, callback: Callback) -> Value<'a, Function> {
        let mut raw_result = ptr::null_mut();
        let status = unsafe {
            sys::napi_create_function(
                self.0,
                name.as_ptr() as *const c_char,
                name.len(),
                Some(callback),
                callback as *mut c_void,
                &mut raw_result,
            )
        };

        debug_assert!(Status::from(status) == Status::Ok);

        Value::from_raw(self, raw_result)
    }

    pub fn define_class<'a, 'b>(&'a self, name: &'b str, constructor_cb: Callback, properties: Vec<Property>) -> Value<'a, Function> {
        let mut raw_result = ptr::null_mut();
        let raw_properties = properties.into_iter().map(|prop| prop.into_raw(self)).collect::<Vec<sys::napi_property_descriptor>>();

        let status = unsafe {
            sys::napi_define_class(
                self.0,
                name.as_ptr() as *const c_char,
                name.len(),
                Some(constructor_cb),
                ptr::null_mut(),
                raw_properties.len(),
                raw_properties.as_ptr(),
                &mut raw_result
            )
        };

        debug_assert!(Status::from(status) == Status::Ok);

        Value::from_raw(self, raw_result)
    }

    pub fn wrap<T: 'static>(&self, js_object: &mut Value<Object>, native_object: T) -> Result<()> {
        let status = unsafe {
            sys::napi_wrap(
                self.0,
                js_object.raw_value,
                Box::into_raw(Box::new(TaggedObject::new(native_object))) as *mut c_void,
                Some(raw_finalize::<T>),
                ptr::null_mut(),
                ptr::null_mut()
            )
        };

        check_status(status).or(Ok(()))
    }

    pub fn unwrap<T: 'static>(&self, js_object: &Value<Object>) -> Result<&mut T> {
        unsafe {
            let mut unknown_tagged_object: *mut c_void = ptr::null_mut();
            let status = sys::napi_unwrap(
                self.0,
                js_object.raw_value,
                &mut unknown_tagged_object
            );
            check_status(status)?;

            let type_id: *const TypeId = mem::transmute(unknown_tagged_object);
            if *type_id == TypeId::of::<T>() {
                let tagged_object: *mut TaggedObject<T> = mem::transmute(unknown_tagged_object);
                Ok(&mut (*tagged_object).object)
            } else {
                Err(Error { status: Status::InvalidArg })
            }
        }
    }

    pub fn async_init(&self, resource: Option<Value<Object>>, name: &str) -> AsyncContext {
        let raw_resource = resource
            .map(|r| r.into_raw())
            .unwrap_or_else(|| self.create_object().into_raw());
        let raw_name = self.create_string(name).into_raw();

        let mut raw_context = ptr::null_mut();
        let mut raw_resource_ref = ptr::null_mut();
        unsafe {
            let status = sys::napi_async_init(self.0, raw_resource, raw_name, &mut raw_context);
            debug_assert!(Status::from(status) == Status::Ok);

            let status = sys::napi_create_reference(self.0, raw_resource, 1, &mut raw_resource_ref);
        }

        AsyncContext { raw_env: self.0, raw_resource: raw_resource_ref, raw_context }
    }

    pub fn create_promise(&self) -> (Value<Object>, Deferred) {
        let mut raw_promise = ptr::null_mut();
        let mut raw_deferred = ptr::null_mut();

        unsafe {
            sys::napi_create_promise(self.0, &mut raw_deferred, &mut raw_promise);
        }

        (Value::from_raw(self, raw_promise), Deferred(raw_deferred))
    }

    pub fn resolve_deferred<T: ValueType>(&self, deferred: Deferred, value: Value<T>) {
        unsafe {
            sys::napi_resolve_deferred(self.0, deferred.0, value.into_raw());
        }
    }

    pub fn spawn<T: 'static + Future>(&self, future: T) {
        let event_loop = unsafe { sys::uv_default_loop() };
        async::spawn(future, event_loop);
    }
}

pub trait ValueType: Copy {
    fn matches_raw_type(raw_type: sys::napi_valuetype) -> bool;
}

impl ValueType for Any {
    fn matches_raw_type(_raw_type: sys::napi_valuetype) -> bool {
        true
    }
}

impl ValueType for Undefined {
    fn matches_raw_type(raw_type: sys::napi_valuetype) -> bool {
        raw_type == sys::napi_valuetype::napi_undefined
    }
}

impl ValueType for Number {
    fn matches_raw_type(raw_type: sys::napi_valuetype) -> bool {
        raw_type == sys::napi_valuetype::napi_number
    }
}

impl ValueType for String {
    fn matches_raw_type(raw_type: sys::napi_valuetype) -> bool {
        raw_type == sys::napi_valuetype::napi_string
    }
}

impl ValueType for Object {
    fn matches_raw_type(raw_type: sys::napi_valuetype) -> bool {
        raw_type == sys::napi_valuetype::napi_object
    }
}

impl ValueType for Function {
    fn matches_raw_type(raw_type: sys::napi_valuetype) -> bool {
        raw_type == sys::napi_valuetype::napi_function
    }
}

impl<'env, T: ValueType> Value<'env, T> {
    pub fn from_raw(env: &'env Env, raw_value: sys::napi_value) -> Self {
        Self { env, raw_value, _marker: PhantomData }
    }

    pub fn into_raw(self) -> sys::napi_value {
        self.raw_value
    }

    pub fn try_into<S: ValueType>(self) -> Result<Value<'env, S>> {
        unsafe {
            let mut value_type: sys::napi_valuetype = mem::uninitialized();
            let status = sys::napi_typeof(self.env.0, self.raw_value, &mut value_type);
            debug_assert!(Status::from(status) == Status::Ok);
            if S::matches_raw_type(value_type) {
                Ok(mem::transmute(self))
            } else {
                Err(Error { status: Status::GenericFailure })
            }
        }
    }

    pub fn coerce_to_number(self) -> Result<Value<'env, Number>> {
        let mut new_raw_value = ptr::null_mut();
        let status = unsafe {
            sys::napi_coerce_to_number(self.env.0, self.raw_value, &mut new_raw_value)
        };
        check_status(status)?;
        Ok(Value {
            env: self.env,
            raw_value: self.raw_value,
            _marker: PhantomData
        })
    }

    pub fn coerce_to_string(self) -> Result<Value<'env, String>> {
        let mut new_raw_value = ptr::null_mut();
        let status = unsafe {
            sys::napi_coerce_to_string(self.env.0, self.raw_value, &mut new_raw_value)
        };
        check_status(status)?;
        Ok(Value {
            env: self.env,
            raw_value: self.raw_value,
            _marker: PhantomData
        })
    }

    pub fn coerce_to_object(self) -> Result<Value<'env, Object>> {
        let mut new_raw_value = ptr::null_mut();
        let status = unsafe {
            sys::napi_coerce_to_object(self.env.0, self.raw_value, (&mut new_raw_value) as *mut sys::napi_value)
        };
        check_status(status)?;
        Ok(Value {
            env: self.env,
            raw_value: self.raw_value,
            _marker: PhantomData
        })
    }
}

impl<'env> Value<'env, String> {
    pub fn len(&self) -> usize {
        let mut raw_length = ptr::null_mut();
        unsafe {
            let status = sys::napi_get_named_property(self.env.0, self.raw_value, "length\0".as_ptr() as *const c_char, &mut raw_length);
            debug_assert!(Status::from(status) == Status::Ok);
        }
        let length: Value<Number> = Value::from_raw(self.env, raw_length);
        length.into()
    }
}

impl<'env> Into<Vec<u16>> for Value<'env, String> {
    fn into(self) -> Vec<u16> {
        let u16_char_count = self.len();
        let mut result = Vec::with_capacity(u16_char_count);

        unsafe {
            let status = sys::napi_get_value_string_utf16(self.env.0, self.raw_value, result.as_mut_ptr(), u16_char_count * 2, &mut 0);
            debug_assert!(Status::from(status) == Status::Ok);
            result.set_len(u16_char_count);
        }

        result
    }
}

impl <'env> Into<usize> for Value<'env, Number> {
    fn into(self) -> usize {
        let mut result = 0;
        let status = unsafe {
            sys::napi_get_value_int64(self.env.0, self.raw_value, &mut result)
        };
        debug_assert!(Status::from(status) == Status::Ok);
        result as usize
    }
}

impl<'env> Into<i64> for Value<'env, Number> {
    fn into(self) -> i64 {
        let mut result = 0;
        let status = unsafe {
            sys::napi_get_value_int64(self.env.0, self.raw_value, &mut result)
        };
        debug_assert!(Status::from(status) == Status::Ok);
        result
    }
}

impl<'env> Value<'env, Object> {
    pub fn set_named_property<'a, T, V: Into<Value<'a, T>>>(&mut self, name: &'a str, value: V) -> Result<()> {
        let key = CString::new(name)?;
        let status = unsafe {
            sys::napi_set_named_property(self.raw_env(), self.raw_value(), key.as_ptr(), value.into().raw_value)
        };
        check_status(status)?;
        Ok(())
    }

    fn raw_value(&self) -> sys::napi_value {
        self.raw_value
    }

    fn raw_env(&self) -> sys::napi_env {
        self.env.0
    }
}

impl AsyncContext {
    pub fn enter<'a, F: 'a + FnOnce(&mut Env)>(&'a self, run_in_context: F) {
        let mut env = Env::from_raw(self.raw_env);
        let mut handle_scope = ptr::null_mut();
        let mut callback_scope = ptr::null_mut();
        let mut raw_resource = ptr::null_mut();

        unsafe {
            sys::napi_open_handle_scope(env.0, &mut handle_scope);
            sys::napi_get_reference_value(env.0, self.raw_resource, &mut raw_resource);
            sys::extras_open_callback_scope(self.raw_context, raw_resource, &mut callback_scope);
        }
        run_in_context(&mut env);
        unsafe {
            sys::extras_close_callback_scope(callback_scope);
            sys::napi_close_handle_scope(env.0, handle_scope);
        }
    }
}

impl Drop for AsyncContext {
    fn drop(&mut self) {
        unsafe {
            sys::napi_delete_reference(self.raw_env, self.raw_resource);
        }
    }
}

impl Property {
    pub fn new(name: &str) -> Self {
        Property {
            name: RustString::from(name),
            raw_descriptor: sys::napi_property_descriptor {
                utf8name: ptr::null_mut(),
                name: ptr::null_mut(),
                method: None,
                getter: None,
                setter: None,
                value: ptr::null_mut(),
                attributes: sys::napi_property_attributes::napi_default,
                data: ptr::null_mut()
            }
        }
    }

    pub fn with_value<T>(mut self, value: Value<T>) -> Self {
        self.raw_descriptor.value = value.raw_value;
        self
    }

    pub fn with_method(mut self, callback: Callback) -> Self {
        self.raw_descriptor.method = Some(callback);
        self
    }

    pub fn with_getter(mut self, callback: Callback) -> Self {
        self.raw_descriptor.getter = Some(callback);
        self
    }

    fn into_raw(mut self, env: &Env) -> sys::napi_property_descriptor {
        self.raw_descriptor.name = env.create_string(&self.name).into_raw();
        self.raw_descriptor
    }
}

impl<T: 'static> TaggedObject<T> {
    fn new(object: T) -> Self {
        TaggedObject {
            type_id: TypeId::of::<T>(),
            object
        }
    }
}

fn check_status(code: sys::napi_status) -> Result<()> {
    let status = Status::from(code);
    match status {
        Status::Ok => Ok(()),
        _ => Err(Error { status })
    }
}

extern "C" fn raw_finalize<T>(_raw_env: sys::napi_env, finalize_data: *mut c_void, _finalize_hint: *mut c_void) {
    unsafe { Box::from_raw(finalize_data as *mut T) };
}
