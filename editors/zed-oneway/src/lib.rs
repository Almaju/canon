use zed_extension_api as zed;

struct OnewayExtension {
    cached_binary_path: Option<String>,
}

impl zed::Extension for OnewayExtension {
    fn new() -> Self {
        Self {
            cached_binary_path: None,
        }
    }

    fn language_server_command(
        &mut self,
        language_server_id: &zed::LanguageServerId,
        worktree: &zed::Worktree,
    ) -> zed::Result<zed::Command> {
        // 1. If already resolved this session, reuse
        if let Some(path) = &self.cached_binary_path {
            return Ok(zed::Command {
                command: path.clone(),
                args: vec!["lsp".to_string()],
                env: Default::default(),
            });
        }

        // 2. Check if `oneway` is on PATH
        if let Some(path) = worktree.which("oneway") {
            self.cached_binary_path = Some(path.clone());
            return Ok(zed::Command {
                command: path,
                args: vec!["lsp".to_string()],
                env: Default::default(),
            });
        }

        // 3. Try to download from GitHub releases
        zed::set_language_server_installation_status(
            language_server_id,
            &zed::LanguageServerInstallationStatus::CheckingForUpdate,
        );

        let release = zed::latest_github_release(
            "Almaju/oneway",
            zed::GithubReleaseOptions {
                require_assets: true,
                pre_release: false,
            },
        );

        if let Ok(release) = release {
            let arch = std::env::consts::ARCH;
            let os = std::env::consts::OS;

            // Look for a matching asset: oneway-{arch}-{os} or similar
            let asset_name = format!("oneway-{}-{}", arch, os);
            let asset = release.assets.iter().find(|a| {
                (a.name.starts_with("oneway-") || a.name == "oneway")
                    && (a.name.contains(&asset_name)
                        || a.name.contains(arch) && a.name.contains(os))
            });

            if let Some(asset) = asset {
                let binary_path = format!("oneway-{}", release.version);

                zed::set_language_server_installation_status(
                    language_server_id,
                    &zed::LanguageServerInstallationStatus::Downloading,
                );

                // Determine file type from extension
                let file_type = if asset.name.ends_with(".tar.gz") || asset.name.ends_with(".tgz") {
                    zed::DownloadedFileType::GzipTar
                } else if asset.name.ends_with(".gz") {
                    zed::DownloadedFileType::Gzip
                } else if asset.name.ends_with(".zip") {
                    zed::DownloadedFileType::Zip
                } else {
                    zed::DownloadedFileType::Uncompressed
                };

                zed::download_file(&asset.download_url, &binary_path, file_type)
                    .map_err(|e| format!("failed to download oneway: {e}"))?;

                // For tar/zip archives, the binary is inside the extracted directory
                let bin_path = if asset.name.ends_with(".tar.gz")
                    || asset.name.ends_with(".tgz")
                    || asset.name.ends_with(".zip")
                {
                    format!("{}/oneway", binary_path)
                } else {
                    binary_path.clone()
                };

                zed::make_file_executable(&bin_path)
                    .map_err(|e| format!("failed to make oneway executable: {e}"))?;

                self.cached_binary_path = Some(bin_path.clone());
                return Ok(zed::Command {
                    command: bin_path,
                    args: vec!["lsp".to_string()],
                    env: Default::default(),
                });
            }
        }

        // 4. Fallback
        Err("oneway not found. Install it with: cargo install --path . (or add it to PATH)".into())
    }
}

zed::register_extension!(OnewayExtension);
