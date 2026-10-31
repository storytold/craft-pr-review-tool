//! `~/.craft_pr_review_config.toml`: this app's own settings. The repo list and
//! GitHub token come from Craft Status's config, which is only ever read here.

use std::path::PathBuf;

use craft_core::config::{normalize_repo, Shortcuts, TrayConfig, WindowConfig};
use craft_core::{Error, Result};
use serde::{Deserialize, Serialize};

const FILE: &str = ".craft_pr_review_config.toml";
pub const TEMPLATE: &str = include_str!("../../../craft_pr_review_config.template.toml");

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub first_party_owners: Vec<String>,
    pub exclude_repos: Vec<String>,
    pub poll_minutes: f64,
    pub full_list_hours: f64,
    pub stale_minutes: f64,
    pub max_points_per_hour: u64,
    pub budget_floor: u64,
    pub frequent_over: u64,
    pub high_quality: u8,
    pub people_toml_is_core: bool,
    pub sensitive_paths: Vec<String>,
    pub claude: ClaudeConfig,
    pub window: WindowConfig,
    pub tray: TrayConfig,
    pub shortcuts: Shortcuts,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClaudeConfig {
    pub enabled: bool,
    pub binary: String,
    pub model: String,
    pub effort: String,
    pub batch_size: usize,
    pub max_runs_per_hour: u32,
    pub max_usd_per_day: f64,
    pub group_refresh_hours: f64,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            first_party_owners: vec!["storytold".into()],
            exclude_repos: vec![],
            poll_minutes: 30.0,
            full_list_hours: 4.0,
            stale_minutes: 90.0,
            max_points_per_hour: 2500,
            budget_floor: 1000,
            frequent_over: 30,
            high_quality: 75,
            people_toml_is_core: true,
            sensitive_paths: [
                ".github/workflows/",
                ".github/actions/",
                ".github/CODEOWNERS",
                "CODEOWNERS",
                "build.rs",
                ".cargo/config",
                "deny.toml",
                ".coderabbit.yaml",
                "sonar-project.properties",
                "Dockerfile",
                "tauri.conf.json",
                "/capabilities/",
            ]
            .map(String::from)
            .to_vec(),
            claude: ClaudeConfig::default(),
            window: WindowConfig {
                width: 1380.0,
                height: 760.0,
                always_on_top: false,
                start_hidden: false,
                opacity: 0.97,
            },
            tray: TrayConfig::default(),
            shortcuts: Shortcuts {
                toggle_window: "CmdOrCtrl+Shift+Y".into(),
            },
        }
    }
}

impl Default for ClaudeConfig {
    fn default() -> Self {
        ClaudeConfig {
            enabled: true,
            binary: String::new(),
            model: "claude-opus-5-5".into(),
            effort: "low".into(),
            batch_size: 12,
            max_runs_per_hour: 12,
            max_usd_per_day: 5.0,
            group_refresh_hours: 24.0,
        }
    }
}

impl Config {
    pub fn path() -> Result<PathBuf> {
        Ok(dirs::home_dir().ok_or(Error::NoHome)?.join(FILE))
    }

    pub fn parse(text: &str) -> Result<Config> {
        let mut c: Config = toml::from_str(text)?;
        c.poll_minutes = c.poll_minutes.clamp(1.0, 24.0 * 60.0);
        c.full_list_hours = c.full_list_hours.max(c.poll_minutes / 60.0);
        c.stale_minutes = c.stale_minutes.max(c.poll_minutes);
        c.window.opacity = c.window.opacity.clamp(0.2, 1.0);
        c.claude.batch_size = c.claude.batch_size.clamp(1, 40);
        Ok(c)
    }

    /// Load the config, writing the template first if there is none.
    pub fn load_or_create() -> Result<Config> {
        let path = Self::path()?;
        if !path.exists() {
            craft_core::atomic_write(&path, TEMPLATE)?;
        }
        let text = std::fs::read_to_string(&path).map_err(|source| Error::Io {
            path: path.display().to_string(),
            source,
        })?;
        Self::parse(&text)
    }

    pub fn poll_secs(&self) -> i64 {
        (self.poll_minutes * 60.0) as i64
    }

    pub fn full_list_secs(&self) -> i64 {
        (self.full_list_hours * 3600.0) as i64
    }

    pub fn stale_secs(&self) -> i64 {
        (self.stale_minutes * 60.0) as i64
    }

    /// `shared_repos` (Craft Status's list) without third-party and excluded repos.
    pub fn watched(&self, shared_repos: &[String]) -> Vec<String> {
        let excluded: Vec<String> = self
            .exclude_repos
            .iter()
            .filter_map(|r| normalize_repo(r))
            .map(|r| r.to_lowercase())
            .collect();
        shared_repos
            .iter()
            .filter(|r| {
                let owner = r.split('/').next().unwrap_or_default();
                self.first_party_owners
                    .iter()
                    .any(|o| o.eq_ignore_ascii_case(owner))
                    && !excluded.contains(&r.to_lowercase())
            })
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_parses_to_defaults() {
        assert_eq!(Config::parse(TEMPLATE).unwrap(), Config::default());
    }

    #[test]
    fn empty_file_is_defaults() {
        assert_eq!(Config::parse("").unwrap(), Config::default());
    }

    #[test]
    fn third_party_and_excluded_repos_are_left_out() {
        let c = Config::parse(r#"exclude_repos = ["https://github.com/storytold/craft-libs"]"#)
            .unwrap();
        let shared = [
            "storytold/photocraft",
            "storytold/craft-libs",
            "tangency-labs/solidcraft",
        ]
        .map(String::from);
        assert_eq!(c.watched(&shared), vec!["storytold/photocraft"]);
    }

    #[test]
    fn its_shortcut_differs_from_craft_status() {
        assert_ne!(
            Config::default().shortcuts.toggle_window,
            craft_core::Config::default().shortcuts.toggle_window
        );
    }

    #[test]
    fn intervals_are_clamped() {
        let c =
            Config::parse("poll_minutes = 0.1\nstale_minutes = 0.5\nfull_list_hours = 0").unwrap();
        assert_eq!(c.poll_secs(), 60);
        assert_eq!(c.stale_secs(), 60);
        assert_eq!(c.full_list_secs(), 60);
    }
}
