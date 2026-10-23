//! Shared names for a Plugin's execution policy, carried by ordinary Options.
//!
//! A Plugin must map these policies to real hardware behavior before declaring
//! the Option. Selecting `Accurate` alone does not change timing or pacing.

use crate::{DebugValue, MachineOptionInfo, MachineOptionKind, NamedValue};

/// The execution policy selected when a Machine is created.
///
/// This is not a wire type. Options carry its lowercase text representation,
/// so introducing this helper does not change the Plugin protocol or ABI.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionMode {
    /// The Machine's documented optimized execution path.
    #[default]
    Fast,
    /// The Machine's documented higher-fidelity execution path.
    Accurate,
}

impl ExecutionMode {
    /// Declare the common Option. The Plugin supplies the hardware-specific
    /// differences in `description`.
    ///
    /// The wire default is absent deliberately: a Plugin can distinguish an
    /// explicit selection from an older, independently layered timing Option.
    /// With neither present, use `ExecutionMode::default()` (fast).
    pub fn option(description: impl Into<String>) -> MachineOptionInfo {
        MachineOptionInfo {
            key: "execution_mode".to_string(),
            flag: "execution-mode".to_string(),
            short: None,
            kind: MachineOptionKind::Enum {
                choices: vec!["fast".to_string(), "accurate".to_string()],
            },
            default: None,
            required: false,
            description: description.into(),
        }
    }

    /// Read an explicit selection, preserving absence for legacy resolution.
    /// Reject malformed requests even when no Host schema validation ran.
    pub fn from_options(options: &[NamedValue]) -> Result<Option<Self>, String> {
        let mut matching = options.iter().filter(|value| value.key == "execution_mode");
        let Some(value) = matching.next() else {
            return Ok(None);
        };
        if matching.next().is_some() {
            return Err("option \"execution_mode\" must not be repeated".to_string());
        }
        match &value.value {
            DebugValue::Text(value) => value.parse().map(Some),
            _ => Err("option \"execution_mode\" must be text (fast or accurate)".to_string()),
        }
    }
}

impl std::str::FromStr for ExecutionMode {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "fast" => Ok(Self::Fast),
            "accurate" => Ok(Self::Accurate),
            other => Err(format!(
                "option \"execution_mode\" must be fast or accurate, not \"{other}\""
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn selected(value: DebugValue) -> NamedValue {
        NamedValue {
            key: "execution_mode".to_string(),
            value,
        }
    }

    #[test]
    fn absence_preserves_legacy_resolution_and_defaults_to_fast() {
        let unrelated = NamedValue {
            key: "mode".to_string(),
            value: DebugValue::Text("c64".to_string()),
        };
        let mode = ExecutionMode::from_options(&[unrelated]).unwrap();
        assert_eq!(mode, None);
        assert_eq!(mode.unwrap_or_default(), ExecutionMode::Fast);
    }

    #[test]
    fn accepts_both_policies() {
        for (text, expected) in [
            ("fast", ExecutionMode::Fast),
            ("accurate", ExecutionMode::Accurate),
        ] {
            let options = [selected(DebugValue::Text(text.to_string()))];
            assert_eq!(
                ExecutionMode::from_options(&options).unwrap(),
                Some(expected)
            );
        }
    }

    #[test]
    fn rejects_unknown_wrongly_typed_and_repeated_selections() {
        for value in [
            DebugValue::Text("slow".to_string()),
            DebugValue::Text("Accurate".to_string()),
            DebugValue::Bool(true),
        ] {
            assert!(ExecutionMode::from_options(&[selected(value)]).is_err());
        }
        let duplicate = selected(DebugValue::Text("fast".to_string()));
        assert!(ExecutionMode::from_options(&[duplicate.clone(), duplicate]).is_err());
    }
}
