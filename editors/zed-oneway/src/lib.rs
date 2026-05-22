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
                args: vec![],
                env: Default::default(),
            });
        }

        // 2. Check if oneway-lsp is on PATH
        if let Some(path) = worktree.which("oneway-lsp") {
            self.cached_binary_path = Some(path.clone());
            return Ok(zed::Command {
                command: path,
                args: vec![],
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

            // Look for a matching asset: oneway-lsp-{arch}-{os} or similar
            let asset_name = format!("oneway-lsp-{}-{}", arch, os);
            let asset = release.assets.iter().find(|a| {
                a.name.contains("oneway-lsp")
                    && (a.name.contains(&asset_name)
                        || a.name.contains(arch) && a.name.contains(os))
            });

            if let Some(asset) = asset {
                let binary_path = format!("oneway-lsp-{}", release.version);

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
                    .map_err(|e| format!("failed to download oneway-lsp: {e}"))?;

                // For tar/zip archives, the binary is inside the extracted directory
                let bin_path = if asset.name.ends_with(".tar.gz")
                    || asset.name.ends_with(".tgz")
                    || asset.name.ends_with(".zip")
                {
                    format!("{}/oneway-lsp", binary_path)
                } else {
                    binary_path.clone()
                };

                zed::make_file_executable(&bin_path)
                    .map_err(|e| format!("failed to make oneway-lsp executable: {e}"))?;

                self.cached_binary_path = Some(bin_path.clone());
                return Ok(zed::Command {
                    command: bin_path,
                    args: vec![],
                    env: Default::default(),
                });
            }
        }

        // 4. Fallback: try the bare name (user might have it installed elsewhere)
        Err("oneway-lsp not found. Install it with: cargo install --path . --bin oneway-lsp".into())
    }
}

zed::register_extension!(OnewayExtension);
