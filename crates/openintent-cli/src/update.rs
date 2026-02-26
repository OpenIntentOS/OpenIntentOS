//! `openintent update` — self-update from GitHub releases.
//!
//! Fetches the latest release from the GitHub API, compares it with the
//! running binary version, and — unless `--check` is passed — downloads and
//! atomically replaces the current executable.

use std::fs;
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

// ---------------------------------------------------------------------------
// GitHub API types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct GithubRelease {
    tag_name: String,
}

// ---------------------------------------------------------------------------
// Platform helpers
// ---------------------------------------------------------------------------

/// Returns the Rust target triple suffix used in release asset names, e.g.
/// `"aarch64-apple-darwin"`.
fn target_triple() -> Result<String> {
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        other => bail!("unsupported CPU architecture: {other}"),
    };

    let platform = match std::env::consts::OS {
        "macos" => "apple-darwin",
        "linux" => "unknown-linux-gnu",
        "windows" => "pc-windows-msvc",
        other => bail!("unsupported operating system: {other}"),
    };

    Ok(format!("{arch}-{platform}"))
}

/// Returns the archive extension for the current OS.
fn archive_ext() -> &'static str {
    if std::env::consts::OS == "windows" {
        "zip"
    } else {
        "tar.gz"
    }
}

// ---------------------------------------------------------------------------
// Version comparison (semver-lite: strip leading 'v', split on '.')
// ---------------------------------------------------------------------------

fn strip_v(s: &str) -> &str {
    s.strip_prefix('v').unwrap_or(s)
}

/// Returns `true` when `remote` is strictly newer than `local`.
fn is_newer(remote: &str, local: &str) -> bool {
    let parse = |s: &str| -> Vec<u64> {
        strip_v(s)
            .split('.')
            .filter_map(|p| p.parse().ok())
            .collect()
    };

    let r = parse(remote);
    let l = parse(local);

    // Compare element by element; treat missing components as 0.
    let len = r.len().max(l.len());
    for i in 0..len {
        let rv = r.get(i).copied().unwrap_or(0);
        let lv = l.get(i).copied().unwrap_or(0);
        if rv != lv {
            return rv > lv;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Binary replacement
// ---------------------------------------------------------------------------

/// Downloads `url` into a temp file and returns the temp file path.
async fn download_to_temp(client: &reqwest::Client, url: &str) -> Result<tempfile::NamedTempFile> {
    println!("  Downloading {url} ...");

    let response = client
        .get(url)
        .send()
        .await
        .context("HTTP request failed")?
        .error_for_status()
        .context("server returned an error status")?;

    let bytes = response
        .bytes()
        .await
        .context("failed to read response body")?;

    let mut tmp = tempfile::NamedTempFile::new().context("failed to create temp file")?;
    tmp.write_all(&bytes)
        .context("failed to write download to temp file")?;

    Ok(tmp)
}

/// Extracts the single `openintent` (or `openintent.exe`) binary from a
/// `.tar.gz` archive and writes it to `dest`.
fn extract_binary_from_targz(archive_path: &std::path::Path, dest: &std::path::Path) -> Result<()> {
    let file = fs::File::open(archive_path).context("failed to open archive")?;
    let gz = flate2::read::GzDecoder::new(file);
    let mut ar = tar::Archive::new(gz);

    let binary_name = if std::env::consts::OS == "windows" {
        "openintent.exe"
    } else {
        "openintent"
    };

    for entry in ar.entries().context("failed to read tar entries")? {
        let mut entry = entry.context("bad tar entry")?;
        let path = entry.path().context("bad entry path")?;

        // Match any path component equal to the binary name.
        let is_binary = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n == binary_name)
            .unwrap_or(false);

        if is_binary {
            entry
                .unpack(dest)
                .context("failed to unpack binary from archive")?;
            return Ok(());
        }
    }

    bail!("binary '{binary_name}' not found inside the release archive")
}

/// Atomically replaces the running executable with the binary at `new_bin`.
///
/// Strategy:
///   1. Copy new binary to `<current_exe>.new` (same filesystem → atomic rename).
///   2. `chmod +x` the temp copy.
///   3. Rename `<current_exe>` → `<current_exe>.old` (allows rollback, also
///      unlocks the path on Linux).
///   4. Rename `<current_exe>.new` → `<current_exe>`.
///   5. Delete `<current_exe>.old`.
fn replace_binary(new_bin: &std::path::Path) -> Result<()> {
    let current_exe = std::env::current_exe().context("failed to locate current executable")?;

    // Resolve symlinks so we write to the real file.
    let current_exe = fs::canonicalize(&current_exe)
        .unwrap_or(current_exe);

    let new_path = current_exe.with_extension("new");
    let old_path = current_exe.with_extension("old");

    // Copy new binary next to the current one (same filesystem).
    fs::copy(new_bin, &new_path).context("failed to copy new binary into place")?;

    // Make executable (Unix only; Windows ignores this).
    #[cfg(unix)]
    {
        let mut perms = fs::metadata(&new_path)
            .context("failed to read new binary metadata")?
            .permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&new_path, perms)
            .context("failed to chmod new binary")?;
    }

    // Rotate: current → old, new → current.
    fs::rename(&current_exe, &old_path)
        .context("failed to move current binary aside")?;
    fs::rename(&new_path, &current_exe)
        .context("failed to move new binary into place")?;

    // Best-effort cleanup of the old binary.
    let _ = fs::remove_file(&old_path);

    Ok(())
}

// ---------------------------------------------------------------------------
// Structured result for programmatic callers (e.g. bot upgrade handler)
// ---------------------------------------------------------------------------

/// Outcome of a check-and-apply update operation.
pub struct UpdateOutcome {
    pub current_version: String,
    pub latest_version: String,
    /// `true` when the binary was successfully replaced.
    pub updated: bool,
}

/// Check for a newer release and, if one exists, download and replace the
/// running binary.  Returns structured info so callers can send custom
/// messages (e.g. the Telegram bot) instead of printing to stdout.
pub async fn check_and_apply_update() -> Result<UpdateOutcome> {
    let current_version = env!("CARGO_PKG_VERSION").to_string();

    let client = reqwest::Client::builder()
        .user_agent(format!("openintent/{current_version}"))
        .build()
        .context("failed to build HTTP client")?;

    let release: GithubRelease = client
        .get("https://api.github.com/repos/OpenIntentOS/OpenIntentOS/releases/latest")
        .send()
        .await
        .context("failed to reach GitHub API")?
        .error_for_status()
        .context("GitHub API returned an error")?
        .json()
        .await
        .context("failed to parse GitHub API response")?;

    let latest_version = release.tag_name.clone();

    if !is_newer(&latest_version, &current_version) {
        return Ok(UpdateOutcome {
            current_version,
            latest_version,
            updated: false,
        });
    }

    // Build download URL for this platform.
    let triple = target_triple().context("could not determine platform target triple")?;
    let ext = archive_ext();
    let asset_name = format!("openintent-{triple}.{ext}");
    let download_url = format!(
        "https://github.com/OpenIntentOS/OpenIntentOS/releases/download/{latest_version}/{asset_name}"
    );

    let archive_tmp = download_to_temp(&client, &download_url).await?;

    let bin_tmp =
        tempfile::NamedTempFile::new().context("failed to create temp file for extracted binary")?;

    extract_binary_from_targz(archive_tmp.path(), bin_tmp.path())
        .context("failed to extract binary from archive")?;

    replace_binary(bin_tmp.path()).context("failed to replace binary")?;

    Ok(UpdateOutcome {
        current_version,
        latest_version,
        updated: true,
    })
}

// ---------------------------------------------------------------------------
// Public command entry point
// ---------------------------------------------------------------------------

/// Implements `openintent update [--check]`.
pub async fn cmd_update(check_only: bool) -> Result<()> {
    let current_version = env!("CARGO_PKG_VERSION");

    println!();
    println!("  OpenIntentOS updater");
    println!("  Current version: v{current_version}");
    println!();

    // ── Fetch latest release from GitHub ────────────────────────────────────
    let client = reqwest::Client::builder()
        .user_agent(format!("openintent/{current_version}"))
        .build()
        .context("failed to build HTTP client")?;

    print!("  Checking for updates... ");
    let _ = std::io::stdout().flush();

    let release: GithubRelease = client
        .get("https://api.github.com/repos/OpenIntentOS/OpenIntentOS/releases/latest")
        .send()
        .await
        .context("failed to reach GitHub API")?
        .error_for_status()
        .context("GitHub API returned an error")?
        .json()
        .await
        .context("failed to parse GitHub API response")?;

    let latest = &release.tag_name;
    println!("done.");
    println!("  Latest version:  {latest}");
    println!();

    // ── Compare versions ─────────────────────────────────────────────────────
    if !is_newer(latest, current_version) {
        println!("  Already running the latest version (v{current_version})");
        println!();
        return Ok(());
    }

    println!("  Update available: v{current_version} → {latest}");
    println!();

    if check_only {
        println!("  Run `openintent update` (without --check) to install.");
        println!();
        return Ok(());
    }

    // ── Build download URL ───────────────────────────────────────────────────
    let triple = target_triple()
        .context("could not determine platform target triple")?;
    let ext = archive_ext();
    let asset_name = format!("openintent-{triple}.{ext}");
    let download_url = format!(
        "https://github.com/OpenIntentOS/OpenIntentOS/releases/download/{latest}/{asset_name}"
    );

    println!("  Platform: {triple}");

    // ── Download archive ─────────────────────────────────────────────────────
    let archive_tmp = download_to_temp(&client, &download_url).await?;

    println!("  Download complete.");

    // ── Extract binary from archive ──────────────────────────────────────────
    let bin_tmp = tempfile::NamedTempFile::new()
        .context("failed to create temp file for extracted binary")?;

    extract_binary_from_targz(archive_tmp.path(), bin_tmp.path())
        .context("failed to extract binary from archive")?;

    println!("  Extraction complete.");

    // ── Replace running binary ───────────────────────────────────────────────
    println!("  Replacing binary...");
    replace_binary(bin_tmp.path()).context("failed to replace binary")?;

    println!();
    println!("  Updated to {latest}. Restart the service to apply.");
    println!();

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_comparison_newer() {
        assert!(is_newer("0.2.0", "0.1.0"));
        assert!(is_newer("v1.0.0", "0.9.99"));
        assert!(is_newer("0.1.1", "0.1.0"));
    }

    #[test]
    fn version_comparison_same_or_older() {
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.0.9", "0.1.0"));
        assert!(!is_newer("v0.1.0", "0.1.0"));
    }

    #[test]
    fn target_triple_non_empty() {
        // Just verify it doesn't error on the build host.
        let t = target_triple();
        assert!(t.is_ok(), "{t:?}");
        let t = t.unwrap();
        assert!(t.contains('-'));
    }
}
