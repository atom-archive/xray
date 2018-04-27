# Xray Browser

This directory packages Xray for use in a web browser. Because browsers don't provide access to the underlying system, when running in a browser, Xray depends on being able to connect to a shared workspace on a remote instance of the `xray_server` executable. This directory contains a [development web server](./script/server) that serves a browser-compatible user interface and proxies connections to `xray_server` over WebSockets.

Assuming you have built Xray with `script/build --release` in the root of this repo, you can present a web-based UI for any Xray instance as follows.

* Start an instance `xray_server` listening for incoming connections on port 8080:
  ```sh
  # Run in the root of the repository (--headless is optional)
  script/xray --listen=8080 --headless your_project_dir
  ```
* Start the development web server:
  ```sh
  xray_browser/script/server
  ```
