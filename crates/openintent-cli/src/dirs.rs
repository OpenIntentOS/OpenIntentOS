//! Application directory layout for OpenIntentOS.
//!
//! All paths are resolved once at startup via [`AppDirs::resolve`] and then
//! passed around as shared state.  The layout follows 方案二 (self-contained
//! install directory):
//!
//! ```text
//! $OPENINTENT_HOME/          (default: ~/.openintentos)
//!   config/
//!     default.toml
//!     IDENTITY.md
//!     SOUL.md
//!   data/
//!     openintent.db
//!     vault.db
//!   skills/
//!     <skill-name>/
//!   output/                  ← skill output files (videos, reports, …)
//!     <skill-name>/
//!   logs/
//!     openintent.log
//!   run/
//!     bot.pid
//!     web.pid
//! ```
//!
//! Every component reads paths from this struct instead of hard-coding them.
//! Override the root with the `OPENINTENT_HOME` environment variable.

use std::path::PathBuf;

/// Resolved application directory paths.
#[derive(Debug, Clone)]
pub struct AppDirs {
    /// Root of the install directory (`$OPENINTENT_HOME`).
    pub home: PathBuf,

    /// `$home/config/` — TOML config, IDENTITY.md, SOUL.md.
    pub config_dir: PathBuf,

    /// `$home/data/` — SQLite databases.
    pub data_dir: PathBuf,

    /// `$home/skills/` — installed skills.
    pub skills_dir: PathBuf,

    /// `$home/output/` — skill output files (videos, reports, downloads).
    pub output_dir: PathBuf,

    /// `$home/logs/` — persistent log files.
    pub log_dir: PathBuf,

    /// `$home/run/` — PID files and sockets.
    pub run_dir: PathBuf,

    // ── derived file paths ──────────────────────────────────────────

    /// `$data_dir/openintent.db`
    pub db_path: PathBuf,

    /// `$data_dir/vault.db`
    #[allow(dead_code)]
    pub vault_path: PathBuf,

    /// `$config_dir/default.toml`
    pub config_file: PathBuf,

    /// `$config_dir/IDENTITY.md`
    pub identity_file: PathBuf,

    /// `$config_dir/SOUL.md`
    pub soul_file: PathBuf,

    /// `$log_dir/openintent.log`
    pub log_file: PathBuf,

    /// `.env` file at `$home/.env`
    pub env_file: PathBuf,
}

impl AppDirs {
    /// Resolve all paths.
    ///
    /// Priority order for the home directory:
    /// 1. `OPENINTENT_HOME` environment variable
    /// 2. `~/.openintentos`
    /// 3. Current working directory (dev fallback when neither is available)
    pub fn resolve() -> Self {
        let home = Self::resolve_home();

        let config_dir = home.join("config");
        let data_dir   = home.join("data");
        let skills_dir = std::env::var("OPENINTENT_SKILLS_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home.join("skills"));
        let output_dir = home.join("output");
        let log_dir    = home.join("logs");
        let run_dir    = home.join("run");

        let db_path      = data_dir.join("openintent.db");
        let vault_path   = data_dir.join("vault.db");
        let config_file  = config_dir.join("default.toml");
        let identity_file = config_dir.join("IDENTITY.md");
        let soul_file    = config_dir.join("SOUL.md");
        let log_file     = log_dir.join("openintent.log");
        let env_file     = home.join(".env");

        Self {
            home,
            config_dir,
            data_dir,
            skills_dir,
            output_dir,
            log_dir,
            run_dir,
            db_path,
            vault_path,
            config_file,
            identity_file,
            soul_file,
            log_file,
            env_file,
        }
    }

    /// Resolve `$OPENINTENT_HOME`, falling back to `~/.openintentos`.
    fn resolve_home() -> PathBuf {
        if let Ok(h) = std::env::var("OPENINTENT_HOME") {
            if !h.is_empty() {
                return PathBuf::from(h);
            }
        }

        // In development (when running `cargo run` from the repo root),
        // look for an `IDENTITY.md` in the local `config/` directory.
        // If found, treat the CWD as the home dir so developers don't
        // need to set OPENINTENT_HOME.
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        if cwd.join("config").join("IDENTITY.md").exists() {
            return cwd;
        }

        // Production: `~/.openintentos`
        #[cfg(not(test))]
        {
            if let Some(home) = home_dir() {
                return home.join(".openintentos");
            }
        }

        // Last resort: CWD
        cwd
    }

    /// Create all required directories.  Silently skips dirs that already exist.
    pub fn ensure_dirs(&self) -> std::io::Result<()> {
        for dir in [
            &self.config_dir,
            &self.data_dir,
            &self.skills_dir,
            &self.output_dir,
            &self.log_dir,
            &self.run_dir,
        ] {
            std::fs::create_dir_all(dir)?;
        }
        Ok(())
    }

    /// Return the output sub-directory for a given skill name.
    ///
    /// e.g. `output_for_skill("video-clip-maker")` → `$output_dir/video-clip-maker/`
    #[allow(dead_code)]
    pub fn output_for_skill(&self, skill_name: &str) -> PathBuf {
        self.output_dir.join(skill_name)
    }

    /// Path to the PID file for a named process (e.g. `"bot"`, `"web"`).
    #[allow(dead_code)]
    pub fn pid_file(&self, name: &str) -> PathBuf {
        self.run_dir.join(format!("{name}.pid"))
    }
}

/// Portable home-directory lookup (avoids a full `dirs` crate dependency).
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(|| {
            // Windows fallback (not primary target but just in case).
            std::env::var_os("USERPROFILE").map(PathBuf::from)
        })
}

// ---------------------------------------------------------------------------
// Global singleton
// ---------------------------------------------------------------------------

use std::sync::OnceLock;

static APP_DIRS: OnceLock<AppDirs> = OnceLock::new();

/// Return the global `AppDirs` singleton, initialising it on first call.
pub fn app_dirs() -> &'static AppDirs {
    APP_DIRS.get_or_init(AppDirs::resolve)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_with_env_override() {
        unsafe {
            std::env::set_var("OPENINTENT_HOME", "/tmp/test-openintent-home");
        }
        let dirs = AppDirs::resolve();
        assert_eq!(dirs.home, PathBuf::from("/tmp/test-openintent-home"));
        assert_eq!(dirs.db_path, PathBuf::from("/tmp/test-openintent-home/data/openintent.db"));
        assert_eq!(dirs.log_file, PathBuf::from("/tmp/test-openintent-home/logs/openintent.log"));
        assert_eq!(dirs.output_dir, PathBuf::from("/tmp/test-openintent-home/output"));
        unsafe { std::env::remove_var("OPENINTENT_HOME"); }
    }

    #[test]
    fn skill_output_subdir() {
        unsafe { std::env::set_var("OPENINTENT_HOME", "/tmp/oi-test"); }
        let dirs = AppDirs::resolve();
        assert_eq!(
            dirs.output_for_skill("video-clip-maker"),
            PathBuf::from("/tmp/oi-test/output/video-clip-maker")
        );
        unsafe { std::env::remove_var("OPENINTENT_HOME"); }
    }

    #[test]
    fn pid_file_path() {
        unsafe { std::env::set_var("OPENINTENT_HOME", "/tmp/oi-test"); }
        let dirs = AppDirs::resolve();
        assert_eq!(dirs.pid_file("bot"), PathBuf::from("/tmp/oi-test/run/bot.pid"));
        unsafe { std::env::remove_var("OPENINTENT_HOME"); }
    }
}
