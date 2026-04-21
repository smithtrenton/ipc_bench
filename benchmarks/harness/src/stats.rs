use serde::Serialize;
use std::time::Instant;

const TARGET_BATCHES_PER_TRIAL: usize = 100;
const MAX_BATCH_SIZE: usize = 100;

#[derive(Clone, Debug, Serialize)]
pub struct TrialSummary {
    pub trial_index: usize,
    pub total_micros: f64,
    pub average_micros: f64,
    pub min_micros: f64,
    pub max_micros: f64,
    pub stddev_micros: f64,
    pub message_rate: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct AggregateSummary {
    pub total_micros: f64,
    pub average_micros: f64,
    pub min_micros: f64,
    pub max_micros: f64,
    pub stddev_micros: f64,
    pub message_rate: f64,
}

#[derive(Clone, Copy, Debug)]
struct BatchMeasurement {
    average_micros: f64,
    operations: usize,
}

pub fn measure_trial<F>(trial_index: usize, message_count: usize, operation: &mut F) -> TrialSummary
where
    F: FnMut(),
{
    let mut batches = Vec::with_capacity(message_count.min(TARGET_BATCHES_PER_TRIAL));
    let batch_size = measurement_batch_size(message_count);
    let mut remaining = message_count;

    while remaining > 0 {
        let operations = remaining.min(batch_size);
        let start = Instant::now();
        for _ in 0..operations {
            operation();
        }
        batches.push(BatchMeasurement {
            average_micros: start.elapsed().as_secs_f64() * 1_000_000.0 / operations as f64,
            operations,
        });
        remaining -= operations;
    }

    summarize_trial(trial_index, &batches)
}

pub fn aggregate_trials(trials: &[TrialSummary], message_count: usize) -> AggregateSummary {
    debug_assert!(!trials.is_empty(), "trials cannot be empty");

    let total_messages = (trials.len() * message_count) as f64;
    let total_micros = trials.iter().map(|trial| trial.total_micros).sum::<f64>();
    let average_micros = total_micros / total_messages;
    let min_micros = trials
        .iter()
        .map(|trial| trial.min_micros)
        .fold(f64::INFINITY, f64::min);
    let max_micros = trials
        .iter()
        .map(|trial| trial.max_micros)
        .fold(f64::NEG_INFINITY, f64::max);
    let per_trial_messages = message_count as f64;
    let variance = trials
        .iter()
        .map(|trial| {
            per_trial_messages
                * (trial.stddev_micros.powi(2) + (trial.average_micros - average_micros).powi(2))
        })
        .sum::<f64>()
        / total_messages;
    let stddev_micros = variance.sqrt();
    let message_rate = if total_micros == 0.0 {
        f64::INFINITY
    } else {
        total_messages / (total_micros / 1_000_000.0)
    };

    AggregateSummary {
        total_micros,
        average_micros,
        min_micros,
        max_micros,
        stddev_micros,
        message_rate,
    }
}

fn measurement_batch_size(message_count: usize) -> usize {
    message_count
        .div_ceil(TARGET_BATCHES_PER_TRIAL)
        .clamp(1, MAX_BATCH_SIZE)
}

fn summarize_trial(trial_index: usize, batches: &[BatchMeasurement]) -> TrialSummary {
    let count = batches.iter().map(|batch| batch.operations).sum::<usize>() as f64;
    let total_micros = batches
        .iter()
        .map(|batch| batch.average_micros * batch.operations as f64)
        .sum::<f64>();
    let average_micros = total_micros / count;
    let min_micros = batches
        .iter()
        .map(|batch| batch.average_micros)
        .fold(f64::INFINITY, f64::min);
    let max_micros = batches
        .iter()
        .map(|batch| batch.average_micros)
        .fold(f64::NEG_INFINITY, f64::max);
    let variance = batches
        .iter()
        .map(|batch| {
            let delta = batch.average_micros - average_micros;
            delta * delta * batch.operations as f64
        })
        .sum::<f64>()
        / count;
    let stddev_micros = variance.sqrt();
    let message_rate = if total_micros == 0.0 {
        f64::INFINITY
    } else {
        count / (total_micros / 1_000_000.0)
    };

    TrialSummary {
        trial_index,
        total_micros,
        average_micros,
        min_micros,
        max_micros,
        stddev_micros,
        message_rate,
    }
}

#[cfg(test)]
mod tests {
    use super::{TrialSummary, aggregate_trials};

    fn approx_equal(left: f64, right: f64) {
        let delta = (left - right).abs();
        assert!(delta < 0.000_1, "left={left}, right={right}, delta={delta}");
    }

    #[test]
    fn aggregates_trial_summaries() {
        let trials = vec![
            TrialSummary {
                trial_index: 1,
                total_micros: 10.0,
                average_micros: 1.0,
                min_micros: 0.5,
                max_micros: 1.5,
                stddev_micros: 0.2,
                message_rate: 100.0,
            },
            TrialSummary {
                trial_index: 2,
                total_micros: 14.0,
                average_micros: 1.4,
                min_micros: 0.4,
                max_micros: 1.8,
                stddev_micros: 0.4,
                message_rate: 140.0,
            },
        ];

        let aggregate = aggregate_trials(&trials, 10);

        approx_equal(aggregate.total_micros, 24.0);
        approx_equal(aggregate.average_micros, 1.2);
        approx_equal(aggregate.min_micros, 0.4);
        approx_equal(aggregate.max_micros, 1.8);
        approx_equal(aggregate.stddev_micros, 0.374_165_738_677_394_17);
        approx_equal(aggregate.message_rate, 833_333.333_333_333_4);
    }
}
