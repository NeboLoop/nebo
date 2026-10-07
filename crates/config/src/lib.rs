mod cli_detect;
mod config;
mod defaults;
mod logging;
pub mod models;
mod settings;

pub use cli_detect::{AllCliStatuses, CliAvailability, CliStatus, detect_all_clis};
pub use config::{
    memory_url, Config, RuntimeConfig, DEFAULT_MAX_UPLOAD_BYTES, DEFAULT_MEMORY_URL,
};
pub use logging::{RotatingFile, log_file};
pub use defaults::{
    appdata_dir, artifact_napp_path, bundled_napps_dir, data_dir, data_dir_overridden, default_data_dir,
    ensure_artifact_dirs, files_dir, packs_dir, workspace_dir,
    ensure_bot_id, ensure_data_dir, ensure_extension_secret, ensure_install_key, is_setup_complete,
    legacy_data_dir, mark_setup_complete, nebo_dir, nebo_roots, plugin_account_dir, plugin_profiles_root,
    read_bot_id, read_extension_secret, read_install_key, user_artifact_path, user_dir, write_bot_id,
};
pub use models::{ModelUpdate, ModelsConfig};
pub use settings::{Settings, load_settings, save_settings};
