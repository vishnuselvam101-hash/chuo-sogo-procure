//! Roles, permissions and the authoritative role → permission matrix.
//! The backend is the only enforcer; the UI may read this matrix to shape navigation.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OrgKind {
    /// Chuo-Sogo itself: internal procurement staff live here.
    Platform,
    Customer,
}

impl OrgKind {
    pub const fn code(self) -> &'static str {
        match self {
            OrgKind::Platform => "platform",
            OrgKind::Customer => "customer",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        match code {
            "platform" => Some(OrgKind::Platform),
            "customer" => Some(OrgKind::Customer),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Role {
    PlatformSuperAdmin,
    ProcurementAdmin,
    ProcurementOperator,
    CustomerAdmin,
    ProcurementManager,
    Requester,
    Finance,
}

impl Role {
    pub const ALL: [Role; 7] = [
        Role::PlatformSuperAdmin,
        Role::ProcurementAdmin,
        Role::ProcurementOperator,
        Role::CustomerAdmin,
        Role::ProcurementManager,
        Role::Requester,
        Role::Finance,
    ];

    pub const fn code(self) -> &'static str {
        match self {
            Role::PlatformSuperAdmin => "platform_super_admin",
            Role::ProcurementAdmin => "procurement_admin",
            Role::ProcurementOperator => "procurement_operator",
            Role::CustomerAdmin => "customer_admin",
            Role::ProcurementManager => "procurement_manager",
            Role::Requester => "requester",
            Role::Finance => "finance",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        Role::ALL.into_iter().find(|r| r.code() == code)
    }

    /// Which kind of organisation a membership with this role may belong to.
    pub const fn org_kind(self) -> OrgKind {
        match self {
            Role::PlatformSuperAdmin | Role::ProcurementAdmin | Role::ProcurementOperator => {
                OrgKind::Platform
            }
            Role::CustomerAdmin | Role::ProcurementManager | Role::Requester | Role::Finance => {
                OrgKind::Customer
            }
        }
    }

    pub const fn is_internal(self) -> bool {
        matches!(self.org_kind(), OrgKind::Platform)
    }
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Permission {
    // Platform scope (only meaningful inside the platform org).
    ManageOrganisations,
    ManageInternalUsers,
    ManageServiceAssignments,
    ViewSystemHealth,
    // Customer-org scope.
    ManageOrgUsers,
    ManageOrgSettings,
    ManageApprovalPolicy,
    ViewAudit,
    SubmitRequest,
    ViewOwnRequests,
    ViewAllRequests,
    ReviewQuotations,
    ApproveRecommendation,
    /// Customer decision that a directory supplier may be recommended for this org.
    ApproveSuppliers,
    ViewOrders,
    ViewFinance,
    ManageInvoices,
    ViewReports,
    // Internal work performed on a customer org (requires a service assignment).
    OperateRequests,
    VerifyProducts,
    ManageSuppliers,
    ManageRfqs,
    RecordQuotations,
    AssignWork,
    ViewInternalNotes,
}

impl Permission {
    pub const fn code(self) -> &'static str {
        match self {
            Permission::ManageOrganisations => "manage_organisations",
            Permission::ManageInternalUsers => "manage_internal_users",
            Permission::ManageServiceAssignments => "manage_service_assignments",
            Permission::ViewSystemHealth => "view_system_health",
            Permission::ManageOrgUsers => "manage_org_users",
            Permission::ManageOrgSettings => "manage_org_settings",
            Permission::ManageApprovalPolicy => "manage_approval_policy",
            Permission::ViewAudit => "view_audit",
            Permission::SubmitRequest => "submit_request",
            Permission::ViewOwnRequests => "view_own_requests",
            Permission::ViewAllRequests => "view_all_requests",
            Permission::ReviewQuotations => "review_quotations",
            Permission::ApproveRecommendation => "approve_recommendation",
            Permission::ApproveSuppliers => "approve_suppliers",
            Permission::ViewOrders => "view_orders",
            Permission::ViewFinance => "view_finance",
            Permission::ManageInvoices => "manage_invoices",
            Permission::ViewReports => "view_reports",
            Permission::OperateRequests => "operate_requests",
            Permission::VerifyProducts => "verify_products",
            Permission::ManageSuppliers => "manage_suppliers",
            Permission::ManageRfqs => "manage_rfqs",
            Permission::RecordQuotations => "record_quotations",
            Permission::AssignWork => "assign_work",
            Permission::ViewInternalNotes => "view_internal_notes",
        }
    }
}

/// Permissions a role grants inside its own organisation.
pub fn home_permissions(role: Role) -> &'static [Permission] {
    use Permission::*;
    match role {
        Role::PlatformSuperAdmin => &[
            ManageOrganisations,
            ManageInternalUsers,
            ManageServiceAssignments,
            ViewSystemHealth,
            ViewAudit,
            ViewReports,
        ],
        Role::ProcurementAdmin => &[
            ManageServiceAssignments,
            ManageSuppliers,
            ViewReports,
            ViewAudit,
        ],
        Role::ProcurementOperator => &[ManageSuppliers],
        Role::CustomerAdmin => &[
            ManageOrgUsers,
            ManageOrgSettings,
            ManageApprovalPolicy,
            ApproveSuppliers,
            ViewAudit,
            SubmitRequest,
            ViewOwnRequests,
            ViewAllRequests,
            ViewOrders,
            ViewReports,
        ],
        Role::ProcurementManager => &[
            SubmitRequest,
            ViewOwnRequests,
            ViewAllRequests,
            ReviewQuotations,
            ApproveRecommendation,
            ApproveSuppliers,
            ViewOrders,
            ViewReports,
        ],
        Role::Requester => &[SubmitRequest, ViewOwnRequests],
        Role::Finance => &[
            ViewAllRequests,
            ViewOrders,
            ViewFinance,
            ManageInvoices,
            ViewReports,
        ],
    }
}

/// Permissions an internal role grants on a customer org it is assigned to.
/// Customer roles grant nothing outside their own org.
/// Note: internal staff never get ApproveRecommendation or ManageOrgUsers on a customer org.
pub fn service_permissions(role: Role) -> &'static [Permission] {
    use Permission::*;
    match role {
        Role::PlatformSuperAdmin => &[ViewAllRequests, ViewAudit],
        Role::ProcurementAdmin => &[
            ViewAllRequests,
            OperateRequests,
            VerifyProducts,
            ManageSuppliers,
            ManageRfqs,
            RecordQuotations,
            AssignWork,
            ViewInternalNotes,
            ViewOrders,
            ViewReports,
        ],
        Role::ProcurementOperator => &[
            ViewAllRequests,
            OperateRequests,
            VerifyProducts,
            ManageRfqs,
            RecordQuotations,
            ViewInternalNotes,
            ViewOrders,
        ],
        Role::CustomerAdmin | Role::ProcurementManager | Role::Requester | Role::Finance => &[],
    }
}

/// How a principal relates to the organisation a request targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// Member of the target org with this role.
    Member(Role),
    /// Internal staff with an active service assignment to the target org.
    Assigned(Role),
    /// No relation: must be treated exactly like a missing resource.
    None,
}

pub fn allows(access: Access, perm: Permission) -> bool {
    match access {
        Access::Member(role) => home_permissions(role).contains(&perm),
        Access::Assigned(role) => role.is_internal() && service_permissions(role).contains(&perm),
        Access::None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_codes_round_trip() {
        for r in Role::ALL {
            assert_eq!(Role::from_code(r.code()), Some(r));
        }
        assert_eq!(Role::from_code("root"), None);
    }

    #[test]
    fn only_customer_roles_can_approve_purchases() {
        for r in Role::ALL {
            let member = allows(Access::Member(r), Permission::ApproveRecommendation);
            let assigned = allows(Access::Assigned(r), Permission::ApproveRecommendation);
            assert!(
                !assigned,
                "{r} must not approve on a customer org via assignment"
            );
            assert_eq!(member, r == Role::ProcurementManager, "{r}");
        }
    }

    #[test]
    fn requester_cannot_approve_or_see_other_requests() {
        let a = Access::Member(Role::Requester);
        assert!(allows(a, Permission::SubmitRequest));
        assert!(!allows(a, Permission::ApproveRecommendation));
        assert!(!allows(a, Permission::ViewAllRequests));
    }

    #[test]
    fn internal_staff_cannot_manage_customer_users() {
        for r in Role::ALL.into_iter().filter(|r| r.is_internal()) {
            assert!(
                !allows(Access::Assigned(r), Permission::ManageOrgUsers),
                "{r}"
            );
        }
    }

    #[test]
    fn customer_roles_grant_nothing_outside_their_org() {
        for r in Role::ALL.into_iter().filter(|r| !r.is_internal()) {
            assert!(service_permissions(r).is_empty(), "{r}");
            assert!(
                !allows(Access::Assigned(r), Permission::ViewAllRequests),
                "{r}"
            );
        }
    }

    #[test]
    fn no_access_means_nothing() {
        assert!(!allows(Access::None, Permission::ViewOwnRequests));
    }

    #[test]
    fn supplier_approval_is_a_customer_decision() {
        for r in Role::ALL {
            assert!(
                !allows(Access::Assigned(r), Permission::ApproveSuppliers),
                "{r}"
            );
            let expected = matches!(r, Role::CustomerAdmin | Role::ProcurementManager);
            assert_eq!(
                allows(Access::Member(r), Permission::ApproveSuppliers),
                expected,
                "{r}"
            );
        }
    }

    #[test]
    fn internal_notes_are_internal_only() {
        for r in Role::ALL.into_iter().filter(|r| !r.is_internal()) {
            assert!(
                !allows(Access::Member(r), Permission::ViewInternalNotes),
                "{r}"
            );
        }
    }
}
