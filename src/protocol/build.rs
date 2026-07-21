use quote::quote;
use schemars::schema::RootSchema;
use std::{env, fs, path::PathBuf};
use typify::{TypeSpace, TypeSpaceSettings};

fn main() {
    let schema_path = "schema/control-wire.schema.json";
    println!("cargo:rerun-if-changed={schema_path}");
    let schema: RootSchema =
        serde_json::from_slice(&fs::read(schema_path).expect("read control wire schema"))
            .expect("parse control wire schema");
    let mut type_space = TypeSpace::new(TypeSpaceSettings::default().with_struct_builder(true));
    type_space
        .add_root_schema(schema)
        .expect("generate Rust types from schema");
    let output = quote! { #type_space }.to_string();
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    fs::write(out.join("generated.rs"), output).expect("write generated Rust types");
}
