//! Agent runtime rules (docs/agents/DESIGN.md): run and exception lifecycles, the human
//! decision on an exception, the monthly model-call budget and the retry schedule.
//! Agents end with proposals; only a person's decision lets the server act on one.

use std::fmt;

/// A run is tried at most this many times (the first attempt included).
pub const MAX_ATTEMPTS: i32 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RunStatus {
    Queued,
    Running,
    Succeeded,
    Failed,
    Skipped,
}

impl RunStatus {
    pub const ALL: [RunStatus; 5] = [
        RunStatus::Queued,
        RunStatus::Running,
        RunStatus::Succeeded,
        RunStatus::Failed,
        RunStatus::Skipped,
    ];

    pub const fn code(self) -> &'static str {
        match self {
            RunStatus::Queued => "queued",
            RunStatus::Running => "running",
            RunStatus::Succeeded => "succeeded",
            RunStatus::Failed => "failed",
            RunStatus::Skipped => "skipped",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.code() == code)
    }

    pub const fn is_final(self) -> bool {
        matches!(
            self,
            RunStatus::Succeeded | RunStatus::Failed | RunStatus::Skipped
        )
    }

    /// Allowed moves: a worker claims a queued run (or re-claims one whose lease ran out),
    /// and ends it, or puts it back in the queue for a retry.
    pub const fn can_move_to(self, to: RunStatus) -> bool {
        matches!(
            (self, to),
            (RunStatus::Queued, RunStatus::Running)
                | (RunStatus::Running, RunStatus::Running)
                | (RunStatus::Running, RunStatus::Queued)
                | (RunStatus::Running, RunStatus::Succeeded)
                | (RunStatus::Running, RunStatus::Failed)
                | (RunStatus::Running, RunStatus::Skipped)
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Trigger {
    Schedule,
    Event,
    Manual,
}

impl Trigger {
    pub const fn code(self) -> &'static str {
        match self {
            Trigger::Schedule => "schedule",
            Trigger::Event => "event",
            Trigger::Manual => "manual",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Severity {
    Info,
    Warning,
    Critical,
}

impl Severity {
    pub const fn code(self) -> &'static str {
        match self {
            Severity::Info => "info",
            Severity::Warning => "warning",
            Severity::Critical => "critical",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        [Severity::Info, Severity::Warning, Severity::Critical]
            .into_iter()
            .find(|s| s.code() == code)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExceptionStatus {
    Open,
    Approved,
    Edited,
    Rejected,
    /// Closed because the situation was handled elsewhere (e.g. a person fixed it by hand).
    ResolvedElsewhere,
}

impl ExceptionStatus {
    pub const ALL: [ExceptionStatus; 5] = [
        ExceptionStatus::Open,
        ExceptionStatus::Approved,
        ExceptionStatus::Edited,
        ExceptionStatus::Rejected,
        ExceptionStatus::ResolvedElsewhere,
    ];

    pub const fn code(self) -> &'static str {
        match self {
            ExceptionStatus::Open => "open",
            ExceptionStatus::Approved => "approved",
            ExceptionStatus::Edited => "edited",
            ExceptionStatus::Rejected => "rejected",
            ExceptionStatus::ResolvedElsewhere => "resolved_elsewhere",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.code() == code)
    }
}

/// A person's answer to an exception. `Edit` carries the action as the person changed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision<A> {
    Approve,
    Edit(A),
    Reject,
}

impl<A> Decision<A> {
    pub const fn code(&self) -> &'static str {
        match self {
            Decision::Approve => "approve",
            Decision::Edit(_) => "edit",
            Decision::Reject => "reject",
        }
    }

    /// The status the exception ends in.
    pub const fn outcome(&self) -> ExceptionStatus {
        match self {
            Decision::Approve => ExceptionStatus::Approved,
            Decision::Edit(_) => ExceptionStatus::Edited,
            Decision::Reject => ExceptionStatus::Rejected,
        }
    }

    /// Whether the server executes an action after this decision.
    pub const fn executes(&self) -> bool {
        !matches!(self, Decision::Reject)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidTransition {
    pub from: ExceptionStatus,
    pub to: ExceptionStatus,
}

impl fmt::Display for InvalidTransition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} -> {}", self.from.code(), self.to.code())
    }
}

/// Only an open exception can be decided or resolved, exactly once.
pub fn close(from: ExceptionStatus, to: ExceptionStatus) -> Result<(), InvalidTransition> {
    if from == ExceptionStatus::Open && to != ExceptionStatus::Open {
        Ok(())
    } else {
        Err(InvalidTransition { from, to })
    }
}

/// Whether another model-calling run fits in this month's budget.
pub fn within_budget(used: i64, budget: i32) -> bool {
    used < i64::from(budget.max(0))
}

/// Seconds to wait before attempt `attempts + 1`, after `attempts` failed: 1, 5, 30 minutes.
pub fn backoff_seconds(attempts: i32) -> i64 {
    match attempts {
        i32::MIN..=1 => 60,
        2 => 300,
        _ => 1800,
    }
}

/// Whether a run that failed on attempt `attempts` is tried again.
pub fn retries(attempts: i32) -> bool {
    attempts < MAX_ATTEMPTS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_round_trip() {
        for s in RunStatus::ALL {
            assert_eq!(RunStatus::from_code(s.code()), Some(s));
        }
        for s in ExceptionStatus::ALL {
            assert_eq!(ExceptionStatus::from_code(s.code()), Some(s));
        }
        for s in [Severity::Info, Severity::Warning, Severity::Critical] {
            assert_eq!(Severity::from_code(s.code()), Some(s));
        }
        assert_eq!(RunStatus::from_code("done"), None);
        assert_eq!(Severity::from_code("high"), None);
    }

    #[test]
    fn runs_end_only_from_running() {
        use RunStatus::*;
        assert!(Queued.can_move_to(Running));
        assert!(Running.can_move_to(Queued));
        for end in [Succeeded, Failed, Skipped] {
            assert!(end.is_final());
            assert!(Running.can_move_to(end));
            assert!(!Queued.can_move_to(end));
            for to in RunStatus::ALL {
                assert!(!end.can_move_to(to), "{end:?} -> {to:?}");
            }
        }
    }

    #[test]
    fn exceptions_are_decided_once() {
        use ExceptionStatus::*;
        let d: Decision<()> = Decision::Edit(());
        assert_eq!(d.outcome(), Edited);
        assert!(d.executes());
        assert!(!Decision::<()>::Reject.executes());
        assert!(close(Open, Approved).is_ok());
        assert!(close(Open, ResolvedElsewhere).is_ok());
        assert!(close(Open, Open).is_err());
        for from in [Approved, Edited, Rejected, ResolvedElsewhere] {
            for to in ExceptionStatus::ALL {
                assert!(close(from, to).is_err(), "{from:?} -> {to:?}");
            }
        }
    }

    #[test]
    fn budget_and_backoff() {
        assert!(within_budget(0, 1));
        assert!(!within_budget(1, 1));
        assert!(!within_budget(0, 0));
        assert!(!within_budget(0, -5));
        assert_eq!(backoff_seconds(1), 60);
        assert_eq!(backoff_seconds(2), 300);
        assert_eq!(backoff_seconds(3), 1800);
        assert!(retries(1) && retries(2) && !retries(3));
    }
}
