//! Process-local host policy, not readiness, trust or an approval grant.
//! Owned desktop launches enable hosting by default; library/standalone server defaults stay off.
use pipkin_engine::config::EngineConfig;

pub const EXECUTE_FLAG: &str = "--experimental-local-claude-cli";
pub const WRITE_FLAG: &str = "--experimental-local-claude-cli-write";
pub const DISABLE_FLAG: &str = "--disable-local-claude-cli";
pub const DISABLE_WRITE_FLAG: &str = "--disable-local-claude-cli-write";

/// Desktop command-line choices are resolved only after demo/attached-host selection.
/// Keeping these separate from the resolved policy preserves duplicate detection and
/// prevents a desktop default from being treated as an attached-server override.
#[derive(Default)]
pub struct DesktopCliOptions {
    legacy: LocalCliOptIn,
    disable: bool,
    disable_write: bool,
}
impl DesktopCliOptions {
    pub fn parse_flag(&mut self, flag: &str, value: Option<&str>) -> Result<bool, String> {
        let slot = match flag {
            DISABLE_FLAG => &mut self.disable,
            DISABLE_WRITE_FLAG => &mut self.disable_write,
            _ => return self.legacy.parse_flag(flag, value),
        };
        if value.is_some() || *slot {
            return Err(format!(
                "{flag} takes no value and may only be specified once"
            ));
        }
        *slot = true;
        Ok(true)
    }
    pub fn explicit(&self) -> bool {
        self.legacy.enabled || self.legacy.write || self.disable || self.disable_write
    }
    pub fn resolve(&self, owned_desktop: bool) -> Result<LocalCliOptIn, String> {
        self.legacy.validate()?;
        if !owned_desktop {
            if self.explicit() {
                return Err("Local CLI host flags require an owned native desktop, not demo or an attached engine".into());
            }
            return Ok(LocalCliOptIn::default());
        }
        if self.legacy.enabled {
            if self.disable || self.disable_write {
                return Err("Do not combine legacy CLI enable flags with disable flags".into());
            }
            // Preserve the meaning of old launch scripts (execute-only was chat-only).
            return Ok(self.legacy);
        }
        Ok(LocalCliOptIn {
            enabled: !self.disable,
            write: !self.disable && !self.disable_write,
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LocalCliOptIn {
    pub enabled: bool,
    pub write: bool,
}
impl LocalCliOptIn {
    pub const fn desktop_default() -> Self {
        Self {
            enabled: true,
            write: true,
        }
    }
    pub fn parse_flag(&mut self, flag: &str, value: Option<&str>) -> Result<bool, String> {
        let slot = match flag {
            EXECUTE_FLAG => &mut self.enabled,
            WRITE_FLAG => &mut self.write,
            _ => return Ok(false),
        };
        if value.is_some() || *slot {
            return Err(format!(
                "{flag} takes no value and may only be specified once"
            ));
        }
        *slot = true;
        Ok(true)
    }
    pub fn validate(self) -> Result<(), String> {
        if self.write && !self.enabled {
            return Err(format!("{WRITE_FLAG} requires {EXECUTE_FLAG}"));
        }
        Ok(())
    }
    pub fn apply(self, config: &mut EngineConfig) -> Result<(), String> {
        self.validate()?;
        config.enable_local_claude_cli = self.enabled;
        config.enable_local_claude_cli_write = self.write;
        Ok(())
    }
    pub fn flags(self) -> Result<Vec<std::ffi::OsString>, String> {
        self.validate()?;
        Ok([(self.enabled, EXECUTE_FLAG), (self.write, WRITE_FLAG)]
            .into_iter()
            .filter(|(on, _)| *on)
            .map(|(_, flag)| flag.into())
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn desktop_defaults_and_opt_outs_never_override_another_host() {
        let defaults = DesktopCliOptions::default();
        assert_eq!(
            defaults.resolve(true).unwrap(),
            LocalCliOptIn::desktop_default()
        );
        assert_eq!(defaults.resolve(false).unwrap(), LocalCliOptIn::default());
        for (flag, expected) in [
            (DISABLE_FLAG, LocalCliOptIn::default()),
            (
                DISABLE_WRITE_FLAG,
                LocalCliOptIn {
                    enabled: true,
                    write: false,
                },
            ),
        ] {
            let mut options = DesktopCliOptions::default();
            assert!(options.parse_flag(flag, None).unwrap());
            assert_eq!(options.resolve(true).unwrap(), expected);
            assert!(options.resolve(false).is_err());
            assert!(options.parse_flag(flag, None).is_err());
            assert!(
                DesktopCliOptions::default()
                    .parse_flag(flag, Some("true"))
                    .is_err()
            );
            options.parse_flag(EXECUTE_FLAG, None).unwrap();
            assert!(options.resolve(true).is_err());
        }
    }

    #[test]
    fn standalone_policy_is_default_off_explicit_and_write_requires_execution() {
        assert!(LocalCliOptIn::default().flags().unwrap().is_empty());
        let mut p = LocalCliOptIn::default();
        assert!(p.parse_flag(WRITE_FLAG, None).unwrap());
        assert!(p.validate().is_err());
        assert!(p.parse_flag(EXECUTE_FLAG, None).unwrap());
        assert_eq!(p.flags().unwrap().len(), 2);
        assert!(p.parse_flag(EXECUTE_FLAG, None).is_err());
        assert!(
            LocalCliOptIn::default()
                .parse_flag(EXECUTE_FLAG, Some("false"))
                .is_err()
        );
        let mut config = EngineConfig::new("unused-data", "unused-config");
        p.apply(&mut config).unwrap();
        assert!(config.enable_local_claude_cli && config.enable_local_claude_cli_write);
        LocalCliOptIn::default().apply(&mut config).unwrap();
        assert!(!config.enable_local_claude_cli && !config.enable_local_claude_cli_write);
    }
}
