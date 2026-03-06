fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Proto root is 2 levels up from crates/napp/
    let workspace_root = "../..";

    // Only compile the protos we actually use as gRPC clients.
    // channel.proto and comm.proto have RPCs named "Connect" which conflict
    // with tonic's generated `connect()` constructor — compile them when needed.
    let protos = &[
        "proto/apps/v0/common.proto",
        "proto/apps/v0/tool.proto",
        "proto/apps/v0/gateway.proto",
    ];

    // Only rebuild if proto files change
    for proto in protos {
        println!("cargo:rerun-if-changed={}/{}", workspace_root, proto);
    }

    let full_paths: Vec<String> = protos
        .iter()
        .map(|p| format!("{}/{}", workspace_root, p))
        .collect();

    tonic_build::configure()
        .build_server(false) // We only need client stubs
        .compile_protos(
            &full_paths,
            // Include path must be the parent so `import "proto/apps/v0/common.proto"` resolves
            &[workspace_root],
        )?;

    Ok(())
}
