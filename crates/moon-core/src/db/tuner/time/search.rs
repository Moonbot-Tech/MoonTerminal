//! Exact circular time-axis optimization with a retained-trade constraint.

use std::collections::VecDeque;

/// Find the most profitable inclusive circular span in linear time after bucketing.
/// Rounded searches consider every outward five-minute boundary pair and score those bounds.
pub(super) fn best_span(
    values: impl Iterator<Item = (usize, f64)>,
    period: usize,
    min_n: usize,
    round: bool,
) -> Option<(u16, u16)> {
    let mut profit = vec![0.0; period];
    let mut count = vec![0usize; period];
    for (minute, pnl) in values {
        profit[minute] += pnl;
        count[minute] += 1;
    }
    let total: f64 = profit.iter().sum();
    let mut sums = vec![0.0; 2 * period + 1];
    let mut counts = vec![0usize; 2 * period + 1];
    for i in 0..2 * period {
        sums[i + 1] = sums[i] + profit[i % period];
        counts[i + 1] = counts[i] + count[i % period];
    }
    let mut best = total;
    let mut answer = None;
    let margin = super::improvement_margin(total);
    let mut eligible = 0;
    let mut starts: VecDeque<usize> = VecDeque::new();
    for end in 0..2 * period - 1 {
        // Prefixes become eligible monotonically as retained count reaches the minimum.
        while eligible < period && eligible <= end && counts[end + 1] - counts[eligible] >= min_n {
            let allowed = if round {
                eligible.is_multiple_of(5)
            } else {
                count[eligible] > 0
            };
            if allowed {
                while starts.back().is_some_and(|&s| sums[s] >= sums[eligible]) {
                    starts.pop_back();
                }
                starts.push_back(eligible);
            }
            eligible += 1;
        }
        while starts.front().is_some_and(|&s| end - s >= period) {
            starts.pop_front();
        }
        let minute = end % period;
        let allowed_end = if round {
            minute.is_multiple_of(5) || minute == period - 1
        } else {
            count[minute] > 0
        };
        if allowed_end {
            if let Some(&start) = starts.front() {
                let pnl = sums[end + 1] - sums[start];
                let shorter_tie = answer.is_some_and(|(f, t)| {
                    pnl == best && (end - start) < (t as usize + period - f as usize) % period
                });
                if pnl > best + margin || shorter_tie {
                    best = pnl;
                    answer = Some((start as u16, minute as u16));
                }
            }
        }
    }
    answer
}

#[cfg(test)]
mod tests;
