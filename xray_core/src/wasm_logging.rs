use wasm_bindgen::prelude::*;

#[wasm_bindgen(js_namespace = console)]
extern "C" {
    pub fn log(s: &str);
    pub fn error(s: &str);
}

#[macro_export]
macro_rules! println {
    ($($arg:tt)*) => ($crate::wasm_logging::log(&::std::fmt::format(format_args!($($arg)*))));
}

#[macro_export]
macro_rules! eprintln {
    ($($arg:tt)*) => ($crate::wasm_logging::error(&::std::fmt::format(format_args!($($arg)*))));
}
