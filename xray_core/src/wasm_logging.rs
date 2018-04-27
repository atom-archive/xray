use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = "console")]
    pub fn log(s: &str);
    #[wasm_bindgen(js_namespace = "console")]
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
