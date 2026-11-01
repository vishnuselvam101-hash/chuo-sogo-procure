//! Customer approval policies: how many distinct human approvals a recommendation needs,
//! by amount. Only customer users with ApproveRecommendation can decide (see rbac);
//! AI and Chuo-Sogo staff never count as approvers.

use std::fmt;

use crate::money::{Currency, Money};

pub const MAX_APPROVALS: u8 = 5;
pub const MAX_TIERS: usize = 10;

/// From `min` (inclusive) upwards, `approvals` distinct approvers are required.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tier {
    pub min: Money,
    pub approvals: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Policy {
    currency: Currency,
    /// Sorted by `min`, strictly increasing.
    tiers: Vec<Tier>,
    allow_self_approval: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyError {
    TooManyTiers,
    InvalidApprovals,
    NegativeThreshold,
    CurrencyMismatch,
    DuplicateThreshold,
    /// A higher amount may never need fewer approvals than a lower one.
    DecreasingApprovals,
}

impl PolicyError {
    pub const fn code(self) -> &'static str {
        match self {
            PolicyError::TooManyTiers => "too_many_tiers",
            PolicyError::InvalidApprovals => "invalid_approvals",
            PolicyError::NegativeThreshold => "negative_threshold",
            PolicyError::CurrencyMismatch => "currency_mismatch",
            PolicyError::DuplicateThreshold => "duplicate_threshold",
            PolicyError::DecreasingApprovals => "decreasing_approvals",
        }
    }
}

impl fmt::Display for PolicyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

impl std::error::Error for PolicyError {}

impl Policy {
    pub fn new(
        currency: Currency,
        mut tiers: Vec<Tier>,
        allow_self_approval: bool,
    ) -> Result<Self, PolicyError> {
        if tiers.len() > MAX_TIERS {
            return Err(PolicyError::TooManyTiers);
        }
        for t in &tiers {
            if t.approvals == 0 || t.approvals > MAX_APPROVALS {
                return Err(PolicyError::InvalidApprovals);
            }
            if t.min.minor() < 0 {
                return Err(PolicyError::NegativeThreshold);
            }
            if t.min.currency() != currency {
                return Err(PolicyError::CurrencyMismatch);
            }
        }
        tiers.sort_by_key(|t| t.min.minor());
        for w in tiers.windows(2) {
            if w[0].min == w[1].min {
                return Err(PolicyError::DuplicateThreshold);
            }
            if w[1].approvals < w[0].approvals {
                return Err(PolicyError::DecreasingApprovals);
            }
        }
        Ok(Policy {
            currency,
            tiers,
            allow_self_approval,
        })
    }

    /// The policy an organisation has before configuring one: one approval, never self.
    pub fn default_for(currency: Currency) -> Self {
        Policy {
            currency,
            tiers: Vec::new(),
            allow_self_approval: false,
        }
    }

    pub fn currency(&self) -> Currency {
        self.currency
    }

    pub fn tiers(&self) -> &[Tier] {
        &self.tiers
    }

    pub fn allow_self_approval(&self) -> bool {
        self.allow_self_approval
    }

    /// Approvals needed for `amount`. At least one, always.
    pub fn required_approvals(&self, amount: Money) -> Result<u8, PolicyError> {
        if amount.currency() != self.currency {
            return Err(PolicyError::CurrencyMismatch);
        }
        Ok(self
            .tiers
            .iter()
            .rev()
            .find(|t| amount.minor() >= t.min.minor())
            .map_or(1, |t| t.approvals))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Approve,
    Reject,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecisionError {
    /// The person who raised the request may not approve it (unless the policy allows).
    SelfApproval,
    /// Each approver counts once.
    AlreadyDecided,
}

impl DecisionError {
    pub const fn code(self) -> &'static str {
        match self {
            DecisionError::SelfApproval => "self_approval_not_allowed",
            DecisionError::AlreadyDecided => "already_decided",
        }
    }
}

/// Validates one decision. Identities are opaque comparable ids (user UUIDs in the server).
pub fn check_decider<Id: PartialEq>(
    allow_self_approval: bool,
    decider: &Id,
    requester: &Id,
    decision: Decision,
    previous_deciders: &[Id],
) -> Result<(), DecisionError> {
    if previous_deciders.contains(decider) {
        return Err(DecisionError::AlreadyDecided);
    }
    // Rejecting your own request is always allowed; approving it only if the policy says so.
    if decision == Decision::Approve && decider == requester && !allow_self_approval {
        return Err(DecisionError::SelfApproval);
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Pending { remaining: u8 },
    Approved,
    Rejected,
}

/// Any rejection rejects; otherwise approved once `required` approvals are recorded.
pub fn outcome(required: u8, decisions: &[Decision]) -> Outcome {
    if decisions.contains(&Decision::Reject) {
        return Outcome::Rejected;
    }
    let approvals = decisions
        .iter()
        .filter(|d| **d == Decision::Approve)
        .count();
    let required = usize::from(required.max(1));
    if approvals >= required {
        Outcome::Approved
    } else {
        Outcome::Pending {
            remaining: u8::try_from(required - approvals).unwrap_or(u8::MAX),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jpy(n: i64) -> Money {
        Money::from_minor(n, Currency::Jpy)
    }

    fn policy() -> Policy {
        Policy::new(
            Currency::Jpy,
            vec![
                Tier {
                    min: jpy(1_000_000),
                    approvals: 3,
                },
                Tier {
                    min: jpy(100_000),
                    approvals: 2,
                },
            ],
            false,
        )
        .unwrap()
    }

    #[test]
    fn thresholds_are_inclusive_and_default_to_one() {
        let p = policy();
        assert_eq!(p.required_approvals(jpy(0)), Ok(1));
        assert_eq!(p.required_approvals(jpy(99_999)), Ok(1));
        assert_eq!(p.required_approvals(jpy(100_000)), Ok(2));
        assert_eq!(p.required_approvals(jpy(999_999)), Ok(2));
        assert_eq!(p.required_approvals(jpy(1_000_000)), Ok(3));
        assert_eq!(p.required_approvals(jpy(i64::MAX)), Ok(3));
    }

    #[test]
    fn default_policy_needs_one_approval() {
        assert_eq!(
            Policy::default_for(Currency::Jpy).required_approvals(jpy(10_000_000)),
            Ok(1)
        );
    }

    #[test]
    fn no_implicit_currency_conversion() {
        let usd = Money::from_minor(100, Currency::Usd);
        assert_eq!(
            policy().required_approvals(usd),
            Err(PolicyError::CurrencyMismatch)
        );
    }

    #[test]
    fn invalid_policies_are_refused() {
        let t = |min, approvals| Tier {
            min: jpy(min),
            approvals,
        };
        let new = |tiers| Policy::new(Currency::Jpy, tiers, false);
        assert_eq!(new(vec![t(0, 0)]), Err(PolicyError::InvalidApprovals));
        assert_eq!(new(vec![t(0, 6)]), Err(PolicyError::InvalidApprovals));
        assert_eq!(new(vec![t(-1, 1)]), Err(PolicyError::NegativeThreshold));
        assert_eq!(
            new(vec![t(5, 1), t(5, 2)]),
            Err(PolicyError::DuplicateThreshold)
        );
        assert_eq!(
            new(vec![t(0, 3), t(10, 1)]),
            Err(PolicyError::DecreasingApprovals)
        );
        assert_eq!(new(vec![t(0, 1); 11]), Err(PolicyError::TooManyTiers));
        assert_eq!(
            Policy::new(
                Currency::Jpy,
                vec![Tier {
                    min: Money::from_minor(1, Currency::Usd),
                    approvals: 1
                }],
                false
            ),
            Err(PolicyError::CurrencyMismatch)
        );
    }

    #[test]
    fn requester_cannot_approve_own_request_by_default() {
        assert_eq!(
            check_decider(false, &1, &1, Decision::Approve, &[]),
            Err(DecisionError::SelfApproval)
        );
        assert_eq!(check_decider(false, &1, &1, Decision::Reject, &[]), Ok(()));
        assert_eq!(check_decider(true, &1, &1, Decision::Approve, &[]), Ok(()));
    }

    #[test]
    fn each_approver_counts_once() {
        assert_eq!(
            check_decider(false, &2, &1, Decision::Approve, &[2]),
            Err(DecisionError::AlreadyDecided)
        );
    }

    #[test]
    fn outcome_rules() {
        use Decision::*;
        assert_eq!(outcome(2, &[]), Outcome::Pending { remaining: 2 });
        assert_eq!(outcome(2, &[Approve]), Outcome::Pending { remaining: 1 });
        assert_eq!(outcome(2, &[Approve, Approve]), Outcome::Approved);
        assert_eq!(outcome(2, &[Approve, Reject]), Outcome::Rejected);
        assert_eq!(outcome(0, &[]), Outcome::Pending { remaining: 1 });
    }
}
