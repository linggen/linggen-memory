use serde::{Deserialize, Serialize};

/// Source profile: metadata about a specific source (repository, folder, etc.)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SourceProfile {
    /// Profile name (e.g., "Default Profile", "Production Config")
    pub profile_name: String,
    /// Brief description of the source
    pub description: String,
}

impl SourceProfile {
    pub fn new(profile_name: String, description: String) -> Self {
        Self {
            profile_name,
            description,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_source_profile_new() {
        let profile = SourceProfile::new("My Project".into(), "A great project".into());
        assert_eq!(profile.profile_name, "My Project");
        assert_eq!(profile.description, "A great project");
    }

    #[test]
    fn test_source_profile_default() {
        let profile = SourceProfile::default();
        assert_eq!(profile.profile_name, "");
        assert_eq!(profile.description, "");
    }

    #[test]
    fn test_source_profile_serde_roundtrip() {
        let profile = SourceProfile::new("Test".into(), "Description".into());
        let json = serde_json::to_string(&profile).unwrap();
        let parsed: SourceProfile = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.profile_name, "Test");
        assert_eq!(parsed.description, "Description");
    }

    #[test]
    fn test_source_profile_clone() {
        let profile = SourceProfile::new("A".into(), "B".into());
        let cloned = profile.clone();
        assert_eq!(cloned.profile_name, "A");
        assert_eq!(cloned.description, "B");
    }

    #[test]
    fn test_source_profile_debug() {
        let profile = SourceProfile::new("X".into(), "Y".into());
        let debug = format!("{:?}", profile);
        assert!(debug.contains("X"));
        assert!(debug.contains("Y"));
    }
}
