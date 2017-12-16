extern crate napi_sys;

pub mod sys {
    pub use napi_sys::*;
}

pub type Result<T> = std::result::Result<T, Error>;

struct Error {
    status: Status
}

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
    HandleScopeMismatch
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
    sys_value: sys::napi_value
}

pub struct Object<'env>(Value<'env>);
pub struct Number<'env>(Value<'env>);

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
    pub fn value_from_sys(&self, sys_value: sys::napi_value) -> Value {
        Value { env: self , sys_value }
    }
}

impl From<sys::napi_env> for Env {
    fn from(env: sys::napi_env) -> Self {
        Env(env)
    }
}

impl Object {
    
}
