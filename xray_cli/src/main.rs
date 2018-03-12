#[macro_use]
extern crate serde_derive;
#[macro_use]
extern crate serde_json;
extern crate docopt;

use std::env;
use std::process::Command;
use std::path::Path;
use docopt::Docopt;

const USAGE: &'static str = "
Xray

Usage:
  xray <path>...
  xray (-h | --help)

Options:
  -h --help     Show this screen.
";

#[derive(Debug, Deserialize)]
struct Args {
    arg_path: Vec<String>,
}

fn main() {
    let args: Args = Docopt::new(USAGE)
                            .and_then(|d| d.deserialize())
                            .unwrap_or_else(|e| e.exit());

    let message = json!({
        "type": "CreateWorkspace",
        "paths": args.arg_path
    });

    if let Ok(src_path) = env::var("XRAY_SRC_PATH") {
        let src_path = Path::new(&src_path);
        let electron_app_path = src_path.join("xray_electron");
        let electron_bin_path = electron_app_path.join("node_modules/.bin/electron");
    
        Command::new(electron_bin_path)
            .arg(electron_app_path)
            .env("XRAY_INITIAL_MESSAGE", message.to_string())
            .spawn()
            .expect("Failed to open Xray app");
    } else {
        eprintln!("Must specify the XRAY_APP_PATH environment variable");
    }
}
