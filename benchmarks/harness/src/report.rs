use std::fmt::Write as _;

use serde::Serialize;

use crate::{
    BenchmarkConfig, OutputFormat,
    stats::{AggregateSummary, TrialSummary, aggregate_trials, measure_trial},
};

#[derive(Clone, Debug, Serialize)]
pub struct BenchmarkReport {
    pub method: String,
    pub child_ready: bool,
    pub config: BenchmarkConfig,
    pub trials: Vec<TrialSummary>,
    pub summary: AggregateSummary,
}

pub fn run_benchmark<F>(
    method: &str,
    config: &BenchmarkConfig,
    child_ready: bool,
    mut operation: F,
) -> BenchmarkReport
where
    F: FnMut(),
{
    for _ in 0..config.warmup_count {
        operation();
    }

    let trials = (0..config.trials)
        .map(|trial_index| measure_trial(trial_index + 1, config.message_count, &mut operation))
        .collect::<Vec<_>>();

    let summary = aggregate_trials(&trials, config.message_count);

    BenchmarkReport {
        method: method.to_owned(),
        child_ready,
        config: config.clone(),
        trials,
        summary,
    }
}

impl BenchmarkReport {
    pub fn render(&self, format: OutputFormat) -> Result<String, serde_json::Error> {
        match format {
            OutputFormat::Text => Ok(self.render_text()),
            OutputFormat::Json => serde_json::to_string_pretty(self),
        }
    }

    fn render_text(&self) -> String {
        let mut output = String::new();

        writeln!(output, "============ RESULTS ================").expect("write to string");
        writeln!(output, "Method:             {}", self.method).expect("write to string");
        writeln!(
            output,
            "Child bootstrap:    {}",
            if self.child_ready { "ok" } else { "not used" }
        )
        .expect("write to string");
        writeln!(output, "Message size:       {}", self.config.message_size)
            .expect("write to string");
        writeln!(output, "Message count:      {}", self.config.message_count)
            .expect("write to string");
        writeln!(output, "Warmup count:       {}", self.config.warmup_count)
            .expect("write to string");
        writeln!(output, "Trial count:        {}", self.config.trials).expect("write to string");
        writeln!(
            output,
            "Total duration:     {:.3}\tms",
            self.summary.total_micros / 1_000.0
        )
        .expect("write to string");
        writeln!(
            output,
            "Average duration:   {:.3}\tus",
            self.summary.average_micros
        )
        .expect("write to string");
        writeln!(
            output,
            "Minimum duration:   {:.3}\tus",
            self.summary.min_micros
        )
        .expect("write to string");
        writeln!(
            output,
            "Maximum duration:   {:.3}\tus",
            self.summary.max_micros
        )
        .expect("write to string");
        writeln!(
            output,
            "Standard deviation: {:.3}\tus",
            self.summary.stddev_micros
        )
        .expect("write to string");
        writeln!(
            output,
            "Message rate:       {:.0}\tmsg/s",
            self.summary.message_rate
        )
        .expect("write to string");

        for trial in &self.trials {
            writeln!(
                output,
                "Trial {:>2}: total {:.3} us | avg {:.3} us | rate {:.0} msg/s",
                trial.trial_index, trial.total_micros, trial.average_micros, trial.message_rate
            )
            .expect("write to string");
        }

        writeln!(output, "=====================================").expect("write to string");

        output
    }
}

#[cfg(test)]
mod tests {
    use crate::{BenchmarkConfig, OutputFormat};

    use super::run_benchmark;

    #[test]
    fn renders_json_report() {
        let config = BenchmarkConfig {
            trials: 1,
            warmup_count: 0,
            output_format: OutputFormat::Json,
            ..BenchmarkConfig::default()
        };
        let report = run_benchmark("placeholder", &config, true, || {});
        let rendered = report
            .render(OutputFormat::Json)
            .expect("json rendering should succeed");

        assert!(rendered.contains("\"method\": \"placeholder\""));
    }
}
