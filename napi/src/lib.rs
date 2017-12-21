extern crate napi_sys;

use std::any::TypeId;
use std::ffi::CString;
use std::os::raw::{c_char, c_void};
use std::mem;
use std::ptr;
use std::string::String as RustString;

pub mod sys {
    pub use napi_sys::*;
}

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

#[derive(Clone, Copy, Debug)]
pub struct Value<'env> {
    env: &'env Env,
    raw_value: sys::napi_value
}

pub struct Number<'env>(Value<'env>);
pub struct String<'env>(Value<'env>);
pub struct Object<'env>(Value<'env>);
pub struct Function<'env>(Value<'env>);

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
                    let env = Env::from(raw_env);
                    let mut exports = Value::from_raw(&env, raw_exports).into_object()
                        .expect("Expected an exports object to be passed to module initializer");

                    let result = $init(&env, &mut exports);

                    match result {
                        Ok(Some(exports)) => exports.into(),
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
            use ::std::ptr;
            use $crate::sys;
            use $crate::{Env, Status, Value};

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

                let env = Env::from(raw_env);
                let this = Value::from_raw(&env, raw_this);
                let mut args: [Value; 8] = unsafe { mem::uninitialized() };
                for (i, raw_arg) in raw_args.into_iter().enumerate() {
                    args[i] = Value::from_raw(&env, *raw_arg)
                }

                let callback = $callback_expr;
                let result = callback(&env, this, &args[0..argc]);

                match result {
                    Ok(Some(result)) => result.into(),
                    Ok(None) => env.get_undefined().into(),
                    Err(e) => {
                        let _ = writeln!(::std::io::stderr(), "Error calling function: {:?}", e);
                        env.get_undefined().into()
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
    pub fn get_undefined<'a>(&'a self) -> Value<'a> {
        let mut raw_value = ptr::null_mut();
        let status = unsafe {
            sys::napi_get_undefined(self.0, &mut raw_value)
        };
        debug_assert!(Status::from(status) == Status::Ok);
        Value::from_raw(self, raw_value)
    }

    pub fn create_int64<'a>(&'a self, int: i64) -> Number<'a> {
        let mut raw_value = ptr::null_mut();
        let status = unsafe {
            sys::napi_create_int64(self.0, int, (&mut raw_value) as *mut sys::napi_value)
        };
        debug_assert!(Status::from(status) == Status::Ok);
        Number::from_raw(self, raw_value)
    }

    pub fn create_string<'a, 'b>(&'a self, s: &'b str) -> String<'a> {
        let mut raw_value = ptr::null_mut();
        let status = unsafe {
            sys::napi_create_string_utf8(self.0, s.as_ptr() as *const c_char, s.len(), &mut raw_value)
        };
        debug_assert!(Status::from(status) == Status::Ok);
        String::from_raw(self, raw_value)
    }

    pub fn create_function<'a>(&self, name: &'a str, callback: Callback) -> Function {
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

        Function::from_raw(self, raw_result)
    }

    pub fn define_class<'a>(&self, name: &'a str, constructor_cb: Callback, properties: Vec<Property>) -> Function {
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

        Function::from_raw(self, raw_result)
    }

    pub fn wrap<T: 'static>(&self, js_object: &mut Value, native_object: T) -> Result<()> {
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

    pub fn unwrap<T: 'static>(&self, js_object: &Value) -> Result<&mut T> {
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
}

impl From<sys::napi_env> for Env {
    fn from(env: sys::napi_env) -> Self {
        Env(env)
    }
}

impl<'env> Value<'env> {
    pub fn from_raw(env: &'env Env, raw_value: sys::napi_value) -> Self {
        Self { env, raw_value }
    }

    pub fn into_object(self) -> Result<Object<'env>> {
        let mut new_raw_value = ptr::null_mut();
        let status = unsafe {
            sys::napi_coerce_to_object(self.env.0, self.raw_value, (&mut new_raw_value) as *mut sys::napi_value)
        };
        check_status(status)?;
        Ok(Object(self))
    }

    pub fn into_number(self) -> Result<Number<'env>> {
        let mut new_raw_value = ptr::null_mut();
        let status = unsafe {
            sys::napi_coerce_to_number(self.env.0, self.raw_value, &mut new_raw_value)
        };
        check_status(status)?;
        Ok(Number(self))
    }
}

impl<'env> Into<sys::napi_value> for Value<'env> {
    fn into(self) -> sys::napi_value {
        self.raw_value
    }
}

impl<'env> Number<'env> {
    fn from_raw(env: &'env Env, raw_value: sys::napi_value) -> Self {
        Number(Value { env, raw_value })
    }
}

impl<'env> Into<Value<'env>> for Number<'env> {
    fn into(self) -> Value<'env> {
        self.0
    }
}

impl <'env> Into<i64> for Number<'env> {
    fn into(self) -> i64 {
        let mut result = 0;
        let status = unsafe {
            sys::napi_get_value_int64(self.0.env.0, self.0.raw_value, &mut result)
        };
        debug_assert!(Status::from(status) == Status::Ok);
        result
    }
}

impl<'env> String<'env> {
    fn from_raw(env: &'env Env, raw_value: sys::napi_value) -> Self {
        String(Value { env, raw_value })
    }
}

impl<'env> Into<Value<'env>> for String<'env> {
    fn into(self) -> Value<'env> {
        self.0
    }
}

impl<'env> Into<sys::napi_value> for String<'env> {
    fn into(self) -> sys::napi_value {
        self.0.raw_value
    }
}

impl <'env> Into<usize> for Number<'env> {
    fn into(self) -> usize {
        let mut result = 0;
        let status = unsafe {
            sys::napi_get_value_int64(self.0.env.0, self.0.raw_value, &mut result)
        };
        debug_assert!(Status::from(status) == Status::Ok);
        result as usize
    }
}

impl<'env> Object<'env> {
    pub fn set_named_property<'a, V: Into<Value<'a>>>(&mut self, name: &'a str, value: V) -> Result<()> {
        let key = CString::new(name)?;
        let status = unsafe {
            sys::napi_set_named_property(self.raw_env(), self.raw_value(), key.as_ptr(), value.into().raw_value)
        };
        check_status(status)?;
        Ok(())
    }

    fn raw_value(&self) -> sys::napi_value {
        self.0.raw_value
    }

    fn raw_env(&self) -> sys::napi_env {
        self.0.env.0
    }
}

impl<'env> Into<sys::napi_value> for Object<'env> {
    fn into(self) -> sys::napi_value {
        self.0.raw_value
    }
}

impl<'env> Into<Value<'env>> for Object<'env> {
    fn into(self) -> Value<'env> {
        self.0
    }
}

impl<'env> Function<'env> {
    fn from_raw(env: &'env Env, raw_value: sys::napi_value) -> Self {
        Function(Value { env, raw_value })
    }
}

impl<'env> Into<Value<'env>> for Function<'env> {
    fn into(self) -> Value<'env> {
        self.0
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

    pub fn with_value(mut self, value: Value) -> Self {
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
        self.raw_descriptor.name = env.create_string(&self.name).into();
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
