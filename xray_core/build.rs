extern crate capnpc;

fn main() {
    capnpc::CompilerCommand::new()
        .file("src/schema.capnp")
        .run()
        .expect("Could not compile Cap'N Proto schema");
}
