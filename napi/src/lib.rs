extern crate napi_sys;
pub extern crate typenum;

use std::ffi::CString;
use std::io::Write;
use std::os::raw::{c_char, c_void};
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
pub struct Function<'env>(Value<'env>);

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

    pub fn create_function<'a, 'b, N, F>(&'a self, name: &'b str, cb_closure: F) -> Function<'a>
        where N: typenum::Unsigned,
              F: for<'c, 'd> Fn(&'c Env, &'c Value, &'d[&'c Value]) -> Result<Option<Value<'c>>>
    {
        let mut raw_result = ptr::null_mut();
        let status = unsafe {
            sys::napi_create_function(
                self.0,
                name.as_ptr() as *const c_char,
                name.len(),
                Some(cb_function::<N, F>),
                &cb_closure as *const _ as *mut c_void,
                &mut raw_result,
            )
        };

        debug_assert!(Status::from(status) == Status::Ok);
        Function::from_raw(self, raw_result)

        extern fn cb_function<N: typenum::Unsigned, F>(raw_env: sys::napi_env, cb_info: sys::napi_callback_info) -> sys::napi_value
            where N: typenum::Unsigned,
                  F: for<'c, 'd> Fn(&'c Env, &'c Value, &'d[&'c Value]) -> Result<Option<Value<'c>>>
        {
            let mut argc = N::to_usize();
            let mut argv = Vec::with_capacity(argc);
            let mut raw_this = ptr::null_mut();
            let mut closure_data: *mut c_void = ptr::null_mut();

            let closure: &mut F = unsafe {
                sys::napi_get_cb_info(
                    raw_env,
                    cb_info,
                    &mut argc,
                    argv.as_mut_ptr(),
                    &mut raw_this,
                    &mut closure_data
                );

                &mut *(closure_data as *mut F)
            };

            let env = Env::from(raw_env);
            let this = Value::from_raw(&env, raw_this);

            let result = closure(&env, &this, &[]);

            match result {
                Ok(Some(result)) => result.into(),
                Ok(None) => env.get_undefined().into(),
                Err(e) => {
                    let _ = writeln!(::std::io::stderr(), "Error calling function: {:?}", e);
                    env.get_undefined().into()
                }
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

fn check_status(code: sys::napi_status) -> Result<()> {
    let status = Status::from(code);
    match status {
        Status::Ok => Ok(()),
        _ => Err(Error { status })
    }
}
