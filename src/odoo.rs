use std::{
    error::Error,
    fs::{self},
    path::{Path, PathBuf},
};

use zed_extension_api::{self as zed, settings::LspSettings, LanguageServerId};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

struct Odoo {
    cached_binary_path: Option<String>,
}

impl Odoo {
    fn detect_odoo_config(&self, worktree: &zed::Worktree) -> Option<PathBuf> {
        // Look for odools.toml in the worktree root and parent directories
        let worktree_path = worktree.root_path();
        let mut current_dir = PathBuf::from(worktree_path);

        // Check current directory and up to 3 parent directories for odools.toml
        for _ in 0..4 {
            let config_path = current_dir.join("odools.toml");
            if config_path.exists() {
                return Some(config_path);
            }

            if let Some(parent) = current_dir.parent() {
                current_dir = parent.to_path_buf();
            } else {
                break;
            }
        }

        None
    }

    fn download_typeshed_from_official_repo(
        &self,
        version_dir: &str,
    ) -> Result<(), Box<dyn Error>> {
        // Download the latest typeshed from the official Python repository
        // Use the main branch archive from GitHub
        let typeshed_url = "https://github.com/python/typeshed/archive/refs/heads/main.zip";

        zed::download_file(typeshed_url, version_dir, zed::DownloadedFileType::Zip)
            .map_err(|err| format!("failed to download typeshed from python/typeshed: {err}"))?;

        // The downloaded archive will be extracted as "typeshed-main", but odoo-ls expects "typeshed"
        // We need to rename the extracted directory
        let extracted_path = Path::new(version_dir).join("typeshed-main");
        let target_path = Path::new(version_dir).join("typeshed");

        if extracted_path.exists() && !target_path.exists() {
            fs::rename(&extracted_path, &target_path)
                .map_err(|err| format!("failed to rename typeshed directory: {err}"))?;
        }

        Ok(())
    }

    fn language_server_binary_path(
        &mut self,
        language_server_id: &LanguageServerId,
        _worktree: &zed::Worktree,
    ) -> Result<String, Box<dyn Error>> {
        if let Some(path) = &self.cached_binary_path {
            if fs::metadata(path).is_ok_and(|stat| stat.is_file()) {
                return Ok(path.clone());
            }
        }

        zed::set_language_server_installation_status(
            language_server_id,
            &zed::LanguageServerInstallationStatus::CheckingForUpdate,
        );

        let version_dir = format!("{}", VERSION);
        fs::create_dir_all(version_dir.clone())
            .map_err(|err| format!("failed to create version directory: {err}"))?;

        let release = zed::github_release_by_tag_name("odoo/odoo-ls", VERSION)?;

        let asset_name = format!("odoo-{}-{}.zip", Odoo::platform(), VERSION);

        let asset = release
            .assets
            .iter()
            .find(|asset| asset.name == asset_name)
            .ok_or_else(|| format!("Odoo: No asset found for asset name {}", asset_name))?;

        // Check for typeshed.zip in the odoo-ls release, if not found we'll download from python/typeshed
        let asset_typeshed = release
            .assets
            .iter()
            .find(|asset| asset.name == "typeshed.zip");

        let mut exe_name = String::from("odoo_ls_server");
        if cfg!(windows) {
            exe_name += ".exe";
        }

        let binary_path = format!(
            "{version_dir}/{bin_name}",
            bin_name = match zed::current_platform().0 {
                zed::Os::Windows => format!("{}.exe", "odoo_ls_server"),
                zed::Os::Mac | zed::Os::Linux => "odoo_ls_server".to_string(),
            }
        );

        if !fs::metadata(&binary_path).is_ok_and(|stat| stat.is_file()) {
            zed::set_language_server_installation_status(
                language_server_id,
                &zed::LanguageServerInstallationStatus::Downloading,
            );

            let path_typeshed = Path::new(&version_dir).join("typeshed");
            if path_typeshed.exists() {
                fs::remove_dir_all(path_typeshed)?;
            }

            zed::download_file(
                &asset.download_url,
                &version_dir,
                zed::DownloadedFileType::Zip,
            )
            .map_err(|err| format!("failed to download file: {err}"))?;

            // Download typeshed - either from odoo-ls release or from python/typeshed
            if let Some(typeshed_asset) = asset_typeshed {
                // Download from odoo-ls release if available
                zed::download_file(
                    &typeshed_asset.download_url,
                    &version_dir,
                    zed::DownloadedFileType::Zip,
                )
                .map_err(|err| format!("failed to download typeshed file: {err}"))?;
            } else {
                // Download latest typeshed from python/typeshed repository
                self.download_typeshed_from_official_repo(&version_dir)?;
            }

            zed::make_file_executable(&binary_path)?;

            let entries = fs::read_dir(".")
                .map_err(|err| format!("failed to list working directory {err}"))?;
            for entry in entries {
                let entry = entry.map_err(|err| format!("failed to load directory entry {err}"))?;
                if entry.file_name().to_str() != Some(&version_dir) {
                    fs::remove_dir_all(entry.path()).ok();
                }
            }
        }

        self.cached_binary_path = Some(binary_path.clone());

        Ok(binary_path)
    }

    fn platform() -> &'static str {
        let (platform, arch) = zed::current_platform();
        match (platform, arch) {
            (zed::Os::Linux, zed::Architecture::X8664) if cfg!(target_env = "musl") => "alpine-x64", // TODO it will never find musl as target_env will always be "" at compilation. Check ldd?
            (zed::Os::Linux, zed::Architecture::Aarch64) if cfg!(target_env = "musl") => {
                "alpine-arm64"
            }
            (zed::Os::Linux, zed::Architecture::X8664) => "linux-x64",
            (zed::Os::Linux, zed::Architecture::Aarch64) => "linux-arm64",
            (zed::Os::Windows, zed::Architecture::X8664) => "win32-x64",
            (zed::Os::Windows, zed::Architecture::Aarch64) => "win32-arm64",
            (zed::Os::Mac, zed::Architecture::X8664) => "darwin-x64",
            (zed::Os::Mac, zed::Architecture::Aarch64) => "darwin-arm64",
            (_os, arch) => {
                // fallback
                println!("Odoo: Warning: Unsupported platform {platform:?}-{arch:?}");
                Box::leak(format!("unknown").into_boxed_str())
            }
        }
    }
}

impl zed::Extension for Odoo {
    fn new() -> Self {
        Self {
            cached_binary_path: None,
        }
    }

    fn language_server_command(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<zed::Command, String> {
        // Detect if we're in a virtual environment and set appropriate environment variables
        let mut env_vars = vec![];

        // Check for virtual environment activation
        if let Ok(virtual_env) = std::env::var("VIRTUAL_ENV") {
            env_vars.push(("VIRTUAL_ENV".to_string(), virtual_env));
        }

        // Preserve PYTHONPATH if set
        if let Ok(python_path) = std::env::var("PYTHONPATH") {
            env_vars.push(("PYTHONPATH".to_string(), python_path));
        }

        Ok(zed::Command {
            command: self
                .language_server_binary_path(language_server_id, worktree)
                .map_err(|e| e.to_string())?,
            args: vec![],
            env: env_vars,
        })
    }

    fn language_server_initialization_options(
        &mut self,
        server_id: &LanguageServerId,
        worktree: &zed_extension_api::Worktree,
    ) -> Result<Option<zed_extension_api::serde_json::Value>, String> {
        let settings = LspSettings::for_worktree(server_id.as_ref(), worktree)
            .ok()
            .and_then(|lsp_settings| lsp_settings.initialization_options.clone())
            .unwrap_or_default();
        Ok(Some(settings))
    }

    fn language_server_workspace_configuration(
        &mut self,
        server_id: &LanguageServerId,
        worktree: &zed_extension_api::Worktree,
    ) -> Result<Option<zed_extension_api::serde_json::Value>, String> {
        let mut settings = LspSettings::for_worktree(server_id.as_ref(), worktree)
            .ok()
            .and_then(|lsp_settings| lsp_settings.settings.clone())
            .unwrap_or_default();

        // Provide intelligent defaults for Odoo development
        if let Some(settings_obj) = settings.as_object_mut() {
            // Add default Odoo settings if not already configured
            if !settings_obj.contains_key("Odoo") {
                // Check if there's an odools.toml config file for better defaults
                let has_config = self.detect_odoo_config(worktree).is_some();

                let default_odoo_config = zed_extension_api::serde_json::json!({
                    "selectedProfile": if has_config { "default" } else { "auto" },
                    "refreshMode": "adaptive",
                    "diagMissingImportsMode": "all"
                });
                settings_obj.insert("Odoo".to_string(), default_odoo_config);
            }
        }

        Ok(Some(settings))
    }
}

zed::register_extension!(Odoo);
