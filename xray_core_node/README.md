# Xray Core Node Bindings

This subproject provides an interface to the `xray-core` library from JavaScript. It builds a shared library which is designed to be loaded as a Node.js compiled add-on.

## Building

Because the target library looks up symbols from Node dynamically, it cannot be built with cargo directly without additional linker flags. See `scripts/build.js` for details.

This project depends on the [`collider`](https://github.com/atom/collider) crate, (which provides a safe interface to Node's N-API. Currently, `collider` is expected to be present as a sibling of the `xray` repository until I take the time to set it up more correctly.
