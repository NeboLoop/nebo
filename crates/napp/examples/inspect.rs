//! Ops one-off: list a .napp's entries and print its plugin.json, so we can
//! verify what a published package actually carries (e.g. auth.profileDirEnv).
fn main() {
    let path = std::env::args().nth(1).expect("usage: inspect <napp>");
    let p = std::path::Path::new(&path);
    for e in nebo_napp::reader::list_napp_entries(p).expect("list") {
        println!("{e}");
    }
    let tmp = std::env::temp_dir().join("napp-inspect-plugin.json");
    if nebo_napp::reader::extract_napp_entry(p, "plugin.json", &tmp).is_ok() {
        println!("--- plugin.json (auth section) ---");
        let s = std::fs::read_to_string(&tmp).unwrap_or_default();
        let v: serde_json::Value = serde_json::from_str(&s).unwrap_or_default();
        println!("{}", serde_json::to_string_pretty(&v["auth"]).unwrap_or_default());
        println!("events: {}", v["events"]);
        let _ = std::fs::remove_file(&tmp);
    }
}
