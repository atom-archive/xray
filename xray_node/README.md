# Xray Core Node Bindings

This subproject provides an interface to the `xray-core` library from JavaScript. It builds a shared library which is designed to be loaded as a Node.js compiled add-on.

## Building

This module is both a Rust crate and an npm module, and it depends on the sibling [`napi`](https://github.com/atom/xray/tree/master/napi) module, which provides a safe interface to Node's N-API. It depends on `napi` both as a Rust crate *and* as an npm module. The npm module provides a special `napi` script that serves as a build harness. Running `npm build` on this module will invoke that build harness to set up required environment variables and linker flags for Cargo.
