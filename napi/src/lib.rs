extern crate napi_sys;

use std::ffi::{CString, NulError};
use std::io::Write;
use std::ptr;

pub mod sys {
    pub use napi_sys::*;
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub struct Error {
    status: Status
}

#[derive(Eq, PartialEq, Debug)]
enum Status {
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
pub struct PropertyDescriptor {
    sys_descriptor: sys::napi_property_descriptor
}

#[derive(Clone, Copy, Debug)]
pub struct Value<'env> {
    env: &'env Env,
    raw_value: sys::napi_value
}

pub struct Number<'env>(Value<'env>);
pub struct Object<'env>(Value<'env>);


pub fn init_module<F>(env: sys::napi_env, exports: sys::napi_value, init: F) -> sys::napi_value
    where F: for <'env> Fn(&'env Env, &'env mut Object) -> Result<Option<Object<'env>>>
{
    let env = Env::from(env);

    let mut exports = Value::from_raw(&env, exports).into_object()
        .expect("Expected an exports object to be passed to module initializer");

    match init(&env, &mut exports) {
        Ok(Some(exports)) => exports.into(),
        Ok(None) => ptr::null_mut(),
        Err(e) => {
            let _ = writeln!(std::io::stderr(), "Error initializing module: {:?}", e);
            ptr::null_mut()
        }
    }
}


impl From<NulError> for Error {
    fn from(error: NulError) -> Self {
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
    pub fn value_from_sys(&self, raw_value: sys::napi_value) -> Value {
        Value { env: self , raw_value }
    }

    pub fn create_int64<'a>(&'a self, int: i64) -> Number<'a> {
        let mut raw_value = ptr::null_mut();
        let status = unsafe {
            sys::napi_create_int64(self.0, int, (&mut raw_value) as *mut sys::napi_value)
        };
        debug_assert!(Status::from(status) == Status::Ok);
        Number::from_raw(self, raw_value)
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

fn check_status(code: sys::napi_status) -> Result<()> {
    let status = Status::from(code);
    match status {
        Status::Ok => Ok(()),
        _ => Err(Error { status })
    }
}
