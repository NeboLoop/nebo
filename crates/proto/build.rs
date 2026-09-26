fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Proto root is 2 levels up from crates/proto/
    let workspace_root = "../..";

    let protos = &[
        "proto/apps/v0/common.proto",
        "proto/apps/v0/tool.proto",
        "proto/apps/v0/gateway.proto",
        "proto/apps/v0/ui.proto",
        "proto/apps/v0/hooks.proto",
        "proto/apps/v0/schedule.proto",
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
        // Nebo only calls sidecars; the server stubs exist for tests that
        // stand in for one (feature `server`).
        .build_server(std::env::var_os("CARGO_FEATURE_SERVER").is_some())
        .compile_protos(
            &full_paths,
            // Include path must be the parent so `import "proto/apps/v0/common.proto"` resolves
            &[workspace_root],
        )?;

    Ok(())
}
