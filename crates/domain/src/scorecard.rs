//! Supplier scorecards (L6): one customer's experience of one supplier in one month, from
//! the customer's own records. Integer basis points (1 bps = 0.01 %), floored; a metric
//! with no samples is `None`, never zero. Pure: the server counts, this computes.
//!
//! - Delivery (on time): orders promised in the month whose first delivery arrived on or
//!   before the promised date, against those that arrived later or are overdue.
//! - Quality: share of the month's invoices that had at least one discrepancy.
//! - Responsiveness: share of RFQs sent in the month that the supplier answered (a
//!   quotation or a decline), and the median hours to the first quotation.
//! - Price: in each RFQ's final comparison, rank 1 of n scores 10 000 and rank n scores 0;
//!   the month's score is the mean. A comparison with one ranked quotation says nothing.

/// What the server counted for one supplier and month.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Inputs {
    pub deliveries_on_time: u32,
    pub deliveries_late: u32,
    pub invoices: u32,
    pub invoices_with_discrepancy: u32,
    pub rfqs_received: u32,
    pub rfqs_answered: u32,
    /// Whole hours from sending the RFQ to the first quotation, one per quotation.
    pub reply_hours: Vec<u32>,
    /// `(rank, ranked_quotations)` in each final comparison the supplier was ranked in.
    pub price_ranks: Vec<(u32, u32)>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Scorecard {
    pub on_time_bps: Option<u32>,
    pub deliveries_measured: u32,
    pub discrepancy_bps: Option<u32>,
    pub invoices: u32,
    pub reply_rate_bps: Option<u32>,
    pub rfqs_received: u32,
    pub median_reply_hours: Option<u32>,
    pub price_score_bps: Option<u32>,
    pub comparisons_ranked: u32,
}

/// `num / den` in basis points, floored; `None` when there is nothing to divide by.
pub fn ratio_bps(num: u32, den: u32) -> Option<u32> {
    if den == 0 {
        return None;
    }
    let bps = u64::from(num.min(den)) * 10_000 / u64::from(den);
    u32::try_from(bps).ok()
}

/// Median, floored mean of the middle two for an even count.
pub fn median(values: &[u32]) -> Option<u32> {
    if values.is_empty() {
        return None;
    }
    let mut v = values.to_vec();
    v.sort_unstable();
    let mid = v.len() / 2;
    Some(if v.len() % 2 == 1 {
        v[mid]
    } else {
        let sum = u64::from(v[mid - 1]) + u64::from(v[mid]);
        u32::try_from(sum / 2).unwrap_or(u32::MAX)
    })
}

/// 10 000 for rank 1, 0 for the last of `of`, linear between; `None` when fewer than two
/// were ranked or the rank is out of range.
pub fn rank_score_bps(rank: u32, of: u32) -> Option<u32> {
    if of < 2 || rank == 0 || rank > of {
        return None;
    }
    let score = u64::from(of - rank) * 10_000 / u64::from(of - 1);
    u32::try_from(score).ok()
}

pub fn compute(i: &Inputs) -> Scorecard {
    let measured = i.deliveries_on_time.saturating_add(i.deliveries_late);
    let scores: Vec<u32> = i
        .price_ranks
        .iter()
        .filter_map(|(r, of)| rank_score_bps(*r, *of))
        .collect();
    let price_score_bps = if scores.is_empty() {
        None
    } else {
        let sum: u64 = scores.iter().map(|s| u64::from(*s)).sum();
        u32::try_from(sum / scores.len() as u64).ok()
    };
    Scorecard {
        on_time_bps: ratio_bps(i.deliveries_on_time, measured),
        deliveries_measured: measured,
        discrepancy_bps: ratio_bps(i.invoices_with_discrepancy, i.invoices),
        invoices: i.invoices,
        reply_rate_bps: ratio_bps(i.rfqs_answered, i.rfqs_received),
        rfqs_received: i.rfqs_received,
        median_reply_hours: median(&i.reply_hours),
        price_score_bps,
        comparisons_ranked: u32::try_from(scores.len()).unwrap_or(u32::MAX),
    }
}

/// When a scorecard asks for a person to review the supplier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Thresholds {
    /// A metric counts only with at least this many samples in the month.
    pub min_samples: u32,
    pub on_time_min_bps: u32,
    pub discrepancy_max_bps: u32,
    pub reply_rate_min_bps: u32,
    /// A fall of at least this much from the previous month also counts.
    pub drop_bps: u32,
}

impl Default for Thresholds {
    fn default() -> Self {
        Thresholds {
            min_samples: 3,
            on_time_min_bps: 8_000,
            discrepancy_max_bps: 2_000,
            reply_rate_min_bps: 5_000,
            drop_bps: 2_000,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ReviewReason {
    LowOnTime,
    OnTimeDropped,
    HighDiscrepancy,
    DiscrepancyRose,
    LowReplyRate,
    ReplyRateDropped,
}

impl ReviewReason {
    pub const fn code(self) -> &'static str {
        match self {
            ReviewReason::LowOnTime => "low_on_time",
            ReviewReason::OnTimeDropped => "on_time_dropped",
            ReviewReason::HighDiscrepancy => "high_discrepancy",
            ReviewReason::DiscrepancyRose => "discrepancy_rose",
            ReviewReason::LowReplyRate => "low_reply_rate",
            ReviewReason::ReplyRateDropped => "reply_rate_dropped",
        }
    }
}

/// Why this month's scorecard needs a review (empty: it does not). `previous` is the month
/// before; changes count only when both months have enough samples.
pub fn review_reasons(
    current: &Scorecard,
    previous: Option<&Scorecard>,
    t: &Thresholds,
) -> Vec<ReviewReason> {
    let enough = |n: u32| n >= t.min_samples;
    let mut out = vec![];
    let on_time = current
        .on_time_bps
        .filter(|_| enough(current.deliveries_measured));
    let prev_on_time =
        previous.and_then(|p| p.on_time_bps.filter(|_| enough(p.deliveries_measured)));
    if let Some(v) = on_time {
        if v < t.on_time_min_bps {
            out.push(ReviewReason::LowOnTime);
        }
        if prev_on_time.is_some_and(|p| p.saturating_sub(v) >= t.drop_bps) {
            out.push(ReviewReason::OnTimeDropped);
        }
    }
    let disc = current.discrepancy_bps.filter(|_| enough(current.invoices));
    let prev_disc = previous.and_then(|p| p.discrepancy_bps.filter(|_| enough(p.invoices)));
    if let Some(v) = disc {
        if v > t.discrepancy_max_bps {
            out.push(ReviewReason::HighDiscrepancy);
        }
        if prev_disc.is_some_and(|p| v.saturating_sub(p) >= t.drop_bps) {
            out.push(ReviewReason::DiscrepancyRose);
        }
    }
    let reply = current
        .reply_rate_bps
        .filter(|_| enough(current.rfqs_received));
    let prev_reply = previous.and_then(|p| p.reply_rate_bps.filter(|_| enough(p.rfqs_received)));
    if let Some(v) = reply {
        if v < t.reply_rate_min_bps {
            out.push(ReviewReason::LowReplyRate);
        }
        if prev_reply.is_some_and(|p| p.saturating_sub(v) >= t.drop_bps) {
            out.push(ReviewReason::ReplyRateDropped);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ratios_are_floored_basis_points_and_empty_means_unknown() {
        assert_eq!(ratio_bps(0, 0), None);
        assert_eq!(ratio_bps(0, 4), Some(0));
        assert_eq!(ratio_bps(2, 3), Some(6_666));
        assert_eq!(ratio_bps(3, 3), Some(10_000));
        // Never above 100 %.
        assert_eq!(ratio_bps(5, 3), Some(10_000));
        assert_eq!(ratio_bps(u32::MAX, u32::MAX), Some(10_000));
    }

    #[test]
    fn median_of_odd_and_even_counts() {
        assert_eq!(median(&[]), None);
        assert_eq!(median(&[7]), Some(7));
        assert_eq!(median(&[30, 2, 5]), Some(5));
        assert_eq!(median(&[1, 2, 4, 100]), Some(3));
        assert_eq!(median(&[1, 2]), Some(1));
        assert_eq!(median(&[u32::MAX, u32::MAX]), Some(u32::MAX));
    }

    #[test]
    fn price_rank_scores_are_linear_from_first_to_last() {
        assert_eq!(rank_score_bps(1, 1), None);
        assert_eq!(rank_score_bps(1, 2), Some(10_000));
        assert_eq!(rank_score_bps(2, 2), Some(0));
        assert_eq!(rank_score_bps(2, 3), Some(5_000));
        assert_eq!(rank_score_bps(2, 4), Some(6_666));
        assert_eq!(rank_score_bps(0, 3), None);
        assert_eq!(rank_score_bps(4, 3), None);
    }

    #[test]
    fn a_month_is_computed_from_its_counts() {
        let card = compute(&Inputs {
            deliveries_on_time: 3,
            deliveries_late: 1,
            invoices: 5,
            invoices_with_discrepancy: 1,
            rfqs_received: 4,
            rfqs_answered: 3,
            reply_hours: vec![48, 2, 10],
            price_ranks: vec![(1, 3), (3, 3), (1, 1)],
        });
        assert_eq!(
            card,
            Scorecard {
                on_time_bps: Some(7_500),
                deliveries_measured: 4,
                discrepancy_bps: Some(2_000),
                invoices: 5,
                reply_rate_bps: Some(7_500),
                rfqs_received: 4,
                median_reply_hours: Some(10),
                // (10 000 + 0) / 2; the sole-bidder comparison does not count.
                price_score_bps: Some(5_000),
                comparisons_ranked: 2,
            }
        );
        let empty = compute(&Inputs::default());
        assert_eq!(empty.on_time_bps, None);
        assert_eq!(empty.discrepancy_bps, None);
        assert_eq!(empty.reply_rate_bps, None);
        assert_eq!(empty.median_reply_hours, None);
        assert_eq!(empty.price_score_bps, None);
    }

    #[test]
    fn reviews_need_enough_samples_and_a_threshold_or_a_drop() {
        let t = Thresholds::default();
        let card = |on, of, disc, inv, rep, rfqs| Scorecard {
            on_time_bps: ratio_bps(on, of),
            deliveries_measured: of,
            discrepancy_bps: ratio_bps(disc, inv),
            invoices: inv,
            reply_rate_bps: ratio_bps(rep, rfqs),
            rfqs_received: rfqs,
            ..Default::default()
        };
        // Good month: nothing.
        assert!(review_reasons(&card(9, 10, 0, 5, 4, 4), None, &t).is_empty());
        // Bad on every metric, but too few samples: nothing.
        assert!(review_reasons(&card(0, 2, 2, 2, 0, 2), None, &t).is_empty());
        // Below thresholds.
        assert_eq!(
            review_reasons(&card(2, 4, 2, 4, 1, 4), None, &t),
            vec![
                ReviewReason::LowOnTime,
                ReviewReason::HighDiscrepancy,
                ReviewReason::LowReplyRate
            ]
        );
        // Exactly on the line is fine: 80 % on time, 20 % discrepancies, 50 % replies.
        assert!(review_reasons(&card(4, 5, 1, 5, 2, 4), None, &t).is_empty());
        // A drop of 20 points from the month before, still above the line.
        let prev = card(10, 10, 0, 5, 4, 4);
        assert_eq!(
            review_reasons(&card(8, 10, 1, 5, 4, 4), Some(&prev), &t),
            vec![ReviewReason::OnTimeDropped, ReviewReason::DiscrepancyRose]
        );
        // A previous month with too few samples is not compared.
        let thin = card(2, 2, 0, 1, 1, 1);
        assert!(review_reasons(&card(8, 10, 0, 5, 4, 4), Some(&thin), &t).is_empty());
        // 19.99 points is not a drop.
        let prev = card(9_999, 10_000, 0, 5, 4, 4);
        assert!(review_reasons(&card(8_000, 10_000, 0, 5, 4, 4), Some(&prev), &t).is_empty());
    }
}
