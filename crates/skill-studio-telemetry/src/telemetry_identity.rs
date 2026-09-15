#[derive(Debug, Clone, Copy)]
pub enum TelemetrySurface {
    Desktop,
    Cli,
    Mcp,
}

impl TelemetrySurface {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Desktop => "desktop",
            Self::Cli => "cli",
            Self::Mcp => "mcp",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum TelemetryEnvironment {
    Development,
    Test,
    Staging,
    Production,
}

#[derive(Debug, Clone)]
pub struct TelemetryIdentity {
    pub(crate) surface: &'static str,
    pub(crate) environment: &'static str,
    pub(crate) release: String,
}

impl TelemetryIdentity {
    pub fn new(
        surface: TelemetrySurface,
        environment: TelemetryEnvironment,
        version: (u32, u32, u32),
    ) -> Self {
        Self {
            surface: surface.name(),
            environment: match environment {
                TelemetryEnvironment::Development => "development",
                TelemetryEnvironment::Test => "test",
                TelemetryEnvironment::Staging => "staging",
                TelemetryEnvironment::Production => "production",
            },
            release: format!("skill-studio@{}.{}.{}", version.0, version.1, version.2),
        }
    }
}

impl TelemetryIdentity {
    pub(crate) fn with_build_revision(mut self, revision: &str) -> Result<Self, &'static str> {
        if !(12..=64).contains(&revision.len())
            || !revision
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err("invalid-build-revision");
        }
        self.release.push('+');
        self.release.push_str(revision);
        Ok(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_identity_is_bounded_and_preserves_version_only_callers() {
        let identity = TelemetryIdentity::new(
            TelemetrySurface::Desktop,
            TelemetryEnvironment::Production,
            (0, 1, 0),
        );
        assert_eq!(identity.release, "skill-studio@0.1.0");
        for revision in ["012345abcdef".to_string(), "a".repeat(64)] {
            let built = identity.clone().with_build_revision(&revision).unwrap();
            assert_eq!(built.release, format!("skill-studio@0.1.0+{revision}"));
            assert_eq!(built.surface, "desktop");
            assert_eq!(built.environment, "production");
        }
        for revision in [
            "".to_string(),
            "a".repeat(11),
            "a".repeat(65),
            "ABCDEF012345".to_string(),
            "/Users/private".to_string(),
            "branch-name".to_string(),
        ] {
            assert!(identity.clone().with_build_revision(&revision).is_err());
        }
    }
}
