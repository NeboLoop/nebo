mod config;
mod cli_detect;
mod defaults;
pub mod models;
mod settings;

pub use config::Config;
pub use cli_detect::{AllCliStatuses, CliAvailability, CliStatus, detect_all_clis};
pub use defaults::{
    artifact_napp_path, bundled_skills_dir, data_dir, ensure_artifact_dirs, ensure_bot_id,
    ensure_data_dir, is_setup_complete, mark_setup_complete, nebo_dir, read_bot_id,
    user_artifact_path, user_dir, write_bot_id,
};
pub use models::{ModelUpdate, ModelsConfig};
pub use settings::{Settings, load_settings, save_settings};
