fn main() -> Result<(), Box<dyn std::error::Error>> {

    println!("cargo:rerun-if-changed=.env");

    tonic_prost_build::configure().compile_protos(
        &["proto/integration.proto", "proto/health.proto"],
        &["proto"],
    )?;

    Ok(())
}
