use std::{env, ffi::OsString};

use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputFormat {
    Text,
    Json,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessRole {
    Parent,
    Child,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct BenchmarkConfig {
    pub message_count: usize,
    pub message_size: usize,
    pub warmup_count: usize,
    pub trials: usize,
    pub output_format: OutputFormat,
    pub role: ProcessRole,
}

impl Default for BenchmarkConfig {
    fn default() -> Self {
        Self {
            message_count: 1_000,
            message_size: 1_000,
            warmup_count: 100,
            trials: 3,
            output_format: OutputFormat::Text,
            role: ProcessRole::Parent,
        }
    }
}

impl BenchmarkConfig {
    pub fn from_env() -> Result<Self, String> {
        Self::from_args(env::args().skip(1))
    }

    pub fn from_args<I>(args: I) -> Result<Self, String>
    where
        I: IntoIterator<Item = String>,
    {
        let mut config = Self::default();
        let args = args.into_iter().collect::<Vec<_>>();
        let mut index = 0;

        while index < args.len() {
            let current = &args[index];

            match current.as_str() {
                "-c" | "--message-count" => {
                    config.message_count =
                        Self::parse_usize(&args, &mut index, current, "message count")?;
                }
                "-s" | "--message-size" => {
                    config.message_size =
                        Self::parse_usize(&args, &mut index, current, "message size")?;
                }
                "-w" | "--warmup-count" => {
                    config.warmup_count =
                        Self::parse_usize(&args, &mut index, current, "warmup count")?;
                }
                "-t" | "--trials" => {
                    config.trials = Self::parse_usize(&args, &mut index, current, "trials")?;
                }
                "--format" => {
                    index += 1;
                    let value = args
                        .get(index)
                        .ok_or_else(|| "missing value for --format".to_owned())?;
                    config.output_format = match value.as_str() {
                        "text" => OutputFormat::Text,
                        "json" => OutputFormat::Json,
                        _ => {
                            return Err(format!(
                                "invalid output format `{value}`; expected `text` or `json`"
                            ));
                        }
                    };
                }
                "--role" => {
                    index += 1;
                    let value = args
                        .get(index)
                        .ok_or_else(|| "missing value for --role".to_owned())?;
                    config.role = match value.as_str() {
                        "parent" => ProcessRole::Parent,
                        "child" => ProcessRole::Child,
                        _ => {
                            return Err(format!(
                                "invalid process role `{value}`; expected `parent` or `child`"
                            ));
                        }
                    };
                }
                "--help" | "-h" => {
                    return Err(Self::usage());
                }
                _ => {
                    return Err(format!(
                        "unrecognized argument `{current}`\n\n{}",
                        Self::usage()
                    ));
                }
            }

            index += 1;
        }

        if config.message_count == 0 {
            return Err("message count must be greater than zero".to_owned());
        }

        if config.trials == 0 {
            return Err("trials must be greater than zero".to_owned());
        }

        Ok(config)
    }

    pub fn child_args(&self) -> Vec<OsString> {
        self.args_for_role(ProcessRole::Child)
    }

    pub fn args_for_role(&self, role: ProcessRole) -> Vec<OsString> {
        let mut args = Vec::with_capacity(12);
        args.extend([
            OsString::from("--message-count"),
            OsString::from(self.message_count.to_string()),
            OsString::from("--message-size"),
            OsString::from(self.message_size.to_string()),
            OsString::from("--warmup-count"),
            OsString::from(self.warmup_count.to_string()),
            OsString::from("--trials"),
            OsString::from(self.trials.to_string()),
            OsString::from("--format"),
            OsString::from(match self.output_format {
                OutputFormat::Text => "text",
                OutputFormat::Json => "json",
            }),
            OsString::from("--role"),
            OsString::from(match role {
                ProcessRole::Parent => "parent",
                ProcessRole::Child => "child",
            }),
        ]);

        args
    }

    pub fn usage() -> String {
        let program_name = env::current_exe()
            .ok()
            .and_then(|path| {
                path.file_stem()
                    .map(|value| value.to_string_lossy().into_owned())
            })
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "ipc-bench".to_owned());

        [
            &format!("Usage: {program_name} [options]"),
            "",
            "Options:",
            "  -c, --message-count <N>  Number of measured round trips (default: 1000)",
            "  -s, --message-size <N>   Payload size in bytes (default: 1000)",
            "  -w, --warmup-count <N>   Warmup iterations before timing (default: 100)",
            "  -t, --trials <N>         Number of benchmark trials (default: 3)",
            "      --format <FORMAT>    Output format: text | json (default: text)",
            "      --role <ROLE>        Internal process role: parent | child",
        ]
        .join("\n")
    }

    fn parse_usize(
        args: &[String],
        index: &mut usize,
        flag: &str,
        label: &str,
    ) -> Result<usize, String> {
        *index += 1;
        let value = args
            .get(*index)
            .ok_or_else(|| format!("missing value for {flag}"))?;

        value
            .parse::<usize>()
            .map_err(|_| format!("invalid {label} `{value}`"))
    }
}

#[cfg(test)]
mod tests {
    use super::{BenchmarkConfig, OutputFormat, ProcessRole};

    #[test]
    fn parses_short_and_long_flags() {
        let config = BenchmarkConfig::from_args([
            "-c".to_owned(),
            "25".to_owned(),
            "--message-size".to_owned(),
            "512".to_owned(),
            "-w".to_owned(),
            "5".to_owned(),
            "--trials".to_owned(),
            "2".to_owned(),
            "--format".to_owned(),
            "json".to_owned(),
            "--role".to_owned(),
            "child".to_owned(),
        ])
        .expect("config should parse");

        assert_eq!(config.message_count, 25);
        assert_eq!(config.message_size, 512);
        assert_eq!(config.warmup_count, 5);
        assert_eq!(config.trials, 2);
        assert_eq!(config.output_format, OutputFormat::Json);
        assert_eq!(config.role, ProcessRole::Child);
    }

    #[test]
    fn child_args_preserve_parent_configuration() {
        let config = BenchmarkConfig {
            message_count: 7,
            message_size: 128,
            warmup_count: 2,
            trials: 4,
            output_format: OutputFormat::Json,
            role: ProcessRole::Parent,
        };

        let child = BenchmarkConfig::from_args(
            config
                .child_args()
                .into_iter()
                .map(|value| value.to_string_lossy().into_owned()),
        )
        .expect("child args should round-trip");

        assert_eq!(child.message_count, 7);
        assert_eq!(child.message_size, 128);
        assert_eq!(child.warmup_count, 2);
        assert_eq!(child.trials, 4);
        assert_eq!(child.output_format, OutputFormat::Json);
        assert_eq!(child.role, ProcessRole::Child);
    }

    #[test]
    fn rejects_zero_message_count() {
        let error = BenchmarkConfig::from_args(["--message-count".to_owned(), "0".to_owned()])
            .expect_err("zero message count should fail");

        assert!(error.contains("message count"));
    }
}
