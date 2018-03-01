# napi

A minimal library for building compiled Node add-ons in Rust.

This library depends on N-API and requires Node 8.9 or later. It is still pretty raw and has not been tested in a production setting.

One nice feature is that this crate allows you to build add-ons purely with the Rust toolchain and without involving `node-gyp`.

## Building

This repository is a Cargo crate *and* an npm module. Any napi-based add-on should also contain *both* `Cargo.toml` to make it a Cargo crate and a `package.json` to make it an npm module.

In your `Cargo.toml` you need to set the `crate-type` to `"cdylib"` so that cargo builds a C-style shared library that can be dynamically loaded by the Node executable. You'll also want to add this crate as a dependency.

```
[lib]
crate-type = ["cdylib"]
```

Building napi-based add-ons directly with `cargo build` isn't recommended, because you'll need to provide a `NODE_INCLUDE_PATH` pointing to the `include` directory for the version of Node you're targeting, as well as some special linker flags that can't be specified in the Cargo configuration.

Instead, you'll want to use the `napi` script, which will be installed automatically at `node_modules/.bin/napi` if you include `napi` as a dependency in your add-on's `package.json`. The napi script supports the following subcommands.

* `napi build [--debug]` Runs `cargo build` with a `NODE_INCLUDE_PATH` based on the path of the Node executable used to run the script and the required linker flags. The optional `--debug` flag will build in debug mode. After building, the script renames the dynamic library to have the `.node` extension to match the convention in the Node.js ecosystem.
* `napi check` Runs `cargo check` with a `NODE_INCLUDE_PATH` based on the Node executable used to run the script.

The `napi` script will be available on the `PATH` of any scripts you define in the `scripts` section of your `package.json`, enabling a setup like this:

```json
{
  "name": "my-add-on",
  "version": "1.0.0",
  "scripts": {
    "build": "napi build",
    "build-debug": "napi build --debug",
    "check": "napi check"
  },
  "dependencies": {
    "napi": "https://github.com/atom/napi"
  }
}
```

So far, the `napi` build script has only been tested on macOS. See the included `test_module` for an example add-on.

## Testing

Because libraries that depend on this crate must be loaded into a Node executable in order to resolve symbols, all tests are written in JavaScript in the `test_module` subdirectory.

To run tests:

```sh
cd test_module
npm run build
npm test
```
