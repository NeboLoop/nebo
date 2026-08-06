//! Diagnostic: run the real skill loader against the real app data dir and
//! print what it sees — the ground truth behind "skill not found".
use std::path::PathBuf;
use std::sync::Arc;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt().with_env_filter("warn,nebo_tools=debug").init();
    let data_dir = PathBuf::from(std::env::var("HOME").unwrap())
        .join("Library/Application Support/Nebo");
    let plugin_store = Arc::new(napp::plugin::PluginStore::new(
        data_dir.join("nebo").join("plugins"),
        data_dir.join("user").join("plugins"),
        None,
    ));
    println!("is_ready(nebo-office) = {}", plugin_store.is_ready("nebo-office"));

    let loader = nebo_tools::skills::Loader::new(
        data_dir.join("nebo").join("skills"),
        data_dir.join("skills"),
    )
    .with_plugin_store(plugin_store);
    loader.load_all().await;
    let skills = loader.list(None).await;
    println!("total skills: {}", skills.len());
    let pptx: Vec<_> = skills.iter().filter(|s| s.name.contains("pptx")).map(|s| &s.name).collect();
    println!("pptx skills: {:?}", pptx);
}
