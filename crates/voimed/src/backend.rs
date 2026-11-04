use anyhow::bail;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum BackendId {
    #[default]
    Cpu,
    Firered,
    Qwen3,
}

impl BackendId {
    pub fn parse(value: &str) -> anyhow::Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "cpu" => Ok(Self::Cpu),
            "firered" => Ok(Self::Firered),
            "qwen3" => Ok(Self::Qwen3),
            other => bail!("Final 后端必须是 cpu、firered 或 qwen3，实际 {other}"),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::Firered => "firered",
            Self::Qwen3 => "qwen3",
        }
    }

    pub fn worker_binary(self) -> Option<&'static str> {
        match self {
            Self::Cpu => None,
            Self::Firered => Some(if cfg!(windows) {
                "voime-gpu-firered.exe"
            } else {
                "voime-gpu-firered"
            }),
            Self::Qwen3 => Some(if cfg!(windows) {
                "voime-gpu-qwen3.exe"
            } else {
                "voime-gpu-qwen3"
            }),
        }
    }

    pub fn model_dirs(self) -> &'static [&'static str] {
        match self {
            Self::Cpu => &[],
            Self::Firered => &["fireredasr2-aed", "fireredpunc"],
            Self::Qwen3 => &["qwen3-asr-1.7b"],
        }
    }
}

impl std::fmt::Display for BackendId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl std::str::FromStr for BackendId {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl serde::Serialize for BackendId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> serde::Deserialize<'de> for BackendId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_three_backends_and_rejects_others() {
        assert_eq!(BackendId::parse("cpu").unwrap(), BackendId::Cpu);
        assert_eq!(BackendId::parse("FIRERED").unwrap(), BackendId::Firered);
        assert_eq!(BackendId::parse("qwen3").unwrap(), BackendId::Qwen3);
        assert!(BackendId::parse("gpu").is_err());
    }
}
