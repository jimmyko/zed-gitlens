use std::fs;
use zed_extension_api::{self as zed, settings::LspSettings, LanguageServerId, Result};

const SERVER_NAME: &str = "gitlens-ls";
const RELEASE_REPO: &str = "jimmyko/zed-gitlens";

struct GitLensExtension {
    cached_binary_path: Option<String>,
}

impl GitLensExtension {
    /// Finds `gitlens-ls`: the `lsp.gitlens-ls.binary.path` setting, then
    /// `$PATH` (for local development), then a binary previously downloaded
    /// from this repository's GitHub releases, otherwise the latest release.
    fn server_binary_path(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<String> {
        let configured_path = LspSettings::for_worktree(SERVER_NAME, worktree)
            .ok()
            .and_then(|settings| settings.binary)
            .and_then(|binary| binary.path);
        if let Some(path) = configured_path {
            return Ok(path);
        }

        if let Some(path) = worktree.which(SERVER_NAME) {
            return Ok(path);
        }

        if let Some(path) = &self.cached_binary_path {
            if fs::metadata(path).is_ok_and(|stat| stat.is_file()) {
                return Ok(path.clone());
            }
        }

        zed::set_language_server_installation_status(
            language_server_id,
            &zed::LanguageServerInstallationStatus::CheckingForUpdate,
        );
        let release = zed::latest_github_release(
            RELEASE_REPO,
            zed::GithubReleaseOptions {
                require_assets: true,
                pre_release: false,
            },
        )
        .map_err(|e| {
            format!(
                "{SERVER_NAME} was not found on PATH and no release could be downloaded ({e}). \
                 Install it with `cargo install --path server` from the extension repo, \
                 or set \"lsp\": {{ \"{SERVER_NAME}\": {{ \"binary\": {{ \"path\": \"...\" }} }} }} \
                 in Zed settings."
            )
        })?;

        let (platform, arch) = zed::current_platform();
        let target = format!(
            "{arch}-{os}",
            arch = match arch {
                zed::Architecture::Aarch64 => "aarch64",
                zed::Architecture::X8664 => "x86_64",
                zed::Architecture::X86 => return Err("32-bit x86 is not supported".into()),
            },
            os = match platform {
                zed::Os::Mac => "apple-darwin",
                zed::Os::Linux => "unknown-linux-gnu",
                zed::Os::Windows => "pc-windows-msvc",
            },
        );
        let (archive_ext, file_type, exe_suffix) = match platform {
            zed::Os::Windows => ("zip", zed::DownloadedFileType::Zip, ".exe"),
            _ => ("tar.gz", zed::DownloadedFileType::GzipTar, ""),
        };
        let asset_name = format!("{SERVER_NAME}-{target}.{archive_ext}");
        let asset = release
            .assets
            .iter()
            .find(|asset| asset.name == asset_name)
            .ok_or_else(|| format!("no release asset named {asset_name}"))?;

        let version_dir = format!("{SERVER_NAME}-{}", release.version);
        let binary_path = format!("{version_dir}/{SERVER_NAME}{exe_suffix}");

        if !fs::metadata(&binary_path).is_ok_and(|stat| stat.is_file()) {
            zed::set_language_server_installation_status(
                language_server_id,
                &zed::LanguageServerInstallationStatus::Downloading,
            );
            zed::download_file(&asset.download_url, &version_dir, file_type)
                .map_err(|e| format!("failed to download {asset_name}: {e}"))?;
            zed::make_file_executable(&binary_path)?;

            // Remove older versions.
            if let Ok(entries) = fs::read_dir(".") {
                for entry in entries.flatten() {
                    if entry.file_name().to_str() != Some(version_dir.as_str()) {
                        fs::remove_dir_all(entry.path()).ok();
                    }
                }
            }
        }

        self.cached_binary_path = Some(binary_path.clone());
        Ok(binary_path)
    }
}

impl zed::Extension for GitLensExtension {
    fn new() -> Self {
        Self {
            cached_binary_path: None,
        }
    }

    fn language_server_command(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<zed::Command> {
        Ok(zed::Command {
            command: self.server_binary_path(language_server_id, worktree)?,
            args: Vec::new(),
            env: worktree.shell_env(),
        })
    }
}

zed::register_extension!(GitLensExtension);
