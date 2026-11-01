//! Procurement request lifecycle (ARCHITECTURE.md §6). The server checks every status
//! change against this table inside the transaction that performs it.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RequestStatus {
    Draft,
    Submitted,
    NeedsClarification,
    Identifying,
    Sourcing,
    RfqSent,
    QuotesReceived,
    UnderComparison,
    Recommended,
    AwaitingApproval,
    Approved,
    Rejected,
    Ordered,
    Delivered,
    Invoiced,
    Disputed,
    Reconciled,
    Closed,
    Cancelled,
}

impl RequestStatus {
    pub const ALL: [RequestStatus; 19] = [
        RequestStatus::Draft,
        RequestStatus::Submitted,
        RequestStatus::NeedsClarification,
        RequestStatus::Identifying,
        RequestStatus::Sourcing,
        RequestStatus::RfqSent,
        RequestStatus::QuotesReceived,
        RequestStatus::UnderComparison,
        RequestStatus::Recommended,
        RequestStatus::AwaitingApproval,
        RequestStatus::Approved,
        RequestStatus::Rejected,
        RequestStatus::Ordered,
        RequestStatus::Delivered,
        RequestStatus::Invoiced,
        RequestStatus::Disputed,
        RequestStatus::Reconciled,
        RequestStatus::Closed,
        RequestStatus::Cancelled,
    ];

    pub const fn code(self) -> &'static str {
        match self {
            RequestStatus::Draft => "draft",
            RequestStatus::Submitted => "submitted",
            RequestStatus::NeedsClarification => "needs_clarification",
            RequestStatus::Identifying => "identifying",
            RequestStatus::Sourcing => "sourcing",
            RequestStatus::RfqSent => "rfq_sent",
            RequestStatus::QuotesReceived => "quotes_received",
            RequestStatus::UnderComparison => "under_comparison",
            RequestStatus::Recommended => "recommended",
            RequestStatus::AwaitingApproval => "awaiting_approval",
            RequestStatus::Approved => "approved",
            RequestStatus::Rejected => "rejected",
            RequestStatus::Ordered => "ordered",
            RequestStatus::Delivered => "delivered",
            RequestStatus::Invoiced => "invoiced",
            RequestStatus::Disputed => "disputed",
            RequestStatus::Reconciled => "reconciled",
            RequestStatus::Closed => "closed",
            RequestStatus::Cancelled => "cancelled",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        RequestStatus::ALL.into_iter().find(|s| s.code() == code)
    }

    /// Items may only be added, changed or removed while the customer still owns the draft
    /// or is answering a clarification.
    pub const fn items_editable(self) -> bool {
        matches!(
            self,
            RequestStatus::Draft | RequestStatus::NeedsClarification
        )
    }
}

impl fmt::Display for RequestStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

/// Which side of the service performs a transition. Permissions are checked separately.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// Customer users acting in their own organisation.
    Customer,
    /// Chuo-Sogo staff acting through a service assignment.
    Operator,
}

/// Every allowed transition and who may perform it.
const TRANSITIONS: &[(RequestStatus, RequestStatus, Side)] = {
    use RequestStatus::*;
    &[
        (Draft, Submitted, Side::Customer),
        (Draft, Cancelled, Side::Customer),
        (Submitted, Cancelled, Side::Customer),
        (Submitted, NeedsClarification, Side::Operator),
        (NeedsClarification, Submitted, Side::Customer),
        (NeedsClarification, Cancelled, Side::Customer),
        (Submitted, Identifying, Side::Operator),
        (Identifying, NeedsClarification, Side::Operator),
        (Identifying, Sourcing, Side::Operator),
        (Sourcing, RfqSent, Side::Operator),
        (RfqSent, QuotesReceived, Side::Operator),
        (QuotesReceived, UnderComparison, Side::Operator),
        // A new comparison run (e.g. after a late quotation) keeps the request where it is.
        (UnderComparison, UnderComparison, Side::Operator),
        (UnderComparison, Recommended, Side::Operator),
        // The operator withdraws an unsent recommendation (e.g. a better quotation arrived).
        (Recommended, UnderComparison, Side::Operator),
        (Recommended, AwaitingApproval, Side::Operator),
        (AwaitingApproval, Approved, Side::Customer),
        (AwaitingApproval, Rejected, Side::Customer),
        (AwaitingApproval, NeedsClarification, Side::Customer),
        (Approved, Ordered, Side::Customer),
        (Ordered, Delivered, Side::Customer),
        // A mistaken receipt is reversed before invoicing.
        (Delivered, Ordered, Side::Customer),
        // Invoices go to the customer (the PO is theirs): their finance team records and
        // reconciles them.
        (Delivered, Invoiced, Side::Customer),
        (Invoiced, Reconciled, Side::Customer),
        (Invoiced, Disputed, Side::Customer),
        (Disputed, Reconciled, Side::Customer),
        (Reconciled, Closed, Side::Customer),
    ]
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidTransition {
    pub from: RequestStatus,
    pub to: RequestStatus,
}

impl fmt::Display for InvalidTransition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "transition {} -> {} not allowed", self.from, self.to)
    }
}

impl std::error::Error for InvalidTransition {}

/// Ok if `side` may move a request from `from` to `to`.
pub fn check(from: RequestStatus, to: RequestStatus, side: Side) -> Result<(), InvalidTransition> {
    if TRANSITIONS
        .iter()
        .any(|&(f, t, s)| f == from && t == to && s == side)
    {
        Ok(())
    } else {
        Err(InvalidTransition { from, to })
    }
}

#[cfg(test)]
mod tests {
    use super::RequestStatus::*;
    use super::*;

    #[test]
    fn codes_round_trip() {
        for s in RequestStatus::ALL {
            assert_eq!(RequestStatus::from_code(s.code()), Some(s));
        }
        assert_eq!(RequestStatus::from_code("paid"), None);
    }

    #[test]
    fn happy_path_is_allowed() {
        let path = [
            (Draft, Submitted, Side::Customer),
            (Submitted, Identifying, Side::Operator),
            (Identifying, Sourcing, Side::Operator),
            (Sourcing, RfqSent, Side::Operator),
            (RfqSent, QuotesReceived, Side::Operator),
            (QuotesReceived, UnderComparison, Side::Operator),
            (UnderComparison, Recommended, Side::Operator),
            (Recommended, AwaitingApproval, Side::Operator),
            (AwaitingApproval, Approved, Side::Customer),
        ];
        for (f, t, s) in path {
            assert_eq!(check(f, t, s), Ok(()), "{f} -> {t}");
        }
    }

    #[test]
    fn operators_never_approve_or_reject() {
        for to in [Approved, Rejected] {
            assert!(check(AwaitingApproval, to, Side::Operator).is_err());
        }
    }

    #[test]
    fn customers_cannot_skip_sourcing() {
        for to in [Identifying, Sourcing, RfqSent, Recommended, Approved] {
            assert!(check(Submitted, to, Side::Customer).is_err(), "{to}");
        }
    }

    #[test]
    fn terminal_states_have_no_exit() {
        for from in [Cancelled, Rejected, Closed] {
            for to in RequestStatus::ALL {
                for side in [Side::Customer, Side::Operator] {
                    assert!(check(from, to, side).is_err(), "{from} -> {to}");
                }
            }
        }
    }

    #[test]
    fn cancel_only_before_work_starts() {
        assert!(check(Draft, Cancelled, Side::Customer).is_ok());
        assert!(check(Submitted, Cancelled, Side::Customer).is_ok());
        for from in [Identifying, Sourcing, RfqSent, AwaitingApproval, Approved] {
            assert!(check(from, Cancelled, Side::Customer).is_err(), "{from}");
        }
    }

    #[test]
    fn items_editable_only_by_customer_states() {
        let editable: Vec<_> = RequestStatus::ALL
            .into_iter()
            .filter(|s| s.items_editable())
            .collect();
        assert_eq!(editable, vec![Draft, NeedsClarification]);
    }
}
