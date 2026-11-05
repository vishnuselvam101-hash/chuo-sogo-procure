//! Roles, permissions and the authoritative role → permission matrix.
//! The backend is the only enforcer; the UI may read this matrix to shape navigation.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OrgKind {
    /// Chuo-Sogo itself: internal procurement staff live here.
    Platform,
    Customer,
    /// A company that lists its products in the catalogue (Phase C).
    Supplier,
    /// One individual shopper's personal account space (Phase E).
    Consumer,
}

impl OrgKind {
    pub const fn code(self) -> &'static str {
        match self {
            OrgKind::Platform => "platform",
            OrgKind::Customer => "customer",
            OrgKind::Supplier => "supplier",
            OrgKind::Consumer => "consumer",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        match code {
            "platform" => Some(OrgKind::Platform),
            "customer" => Some(OrgKind::Customer),
            "supplier" => Some(OrgKind::Supplier),
            "consumer" => Some(OrgKind::Consumer),
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
    SupplierAdmin,
    SupplierStaff,
    Consumer,
}

impl Role {
    pub const ALL: [Role; 10] = [
        Role::PlatformSuperAdmin,
        Role::ProcurementAdmin,
        Role::ProcurementOperator,
        Role::CustomerAdmin,
        Role::ProcurementManager,
        Role::Requester,
        Role::Finance,
        Role::SupplierAdmin,
        Role::SupplierStaff,
        Role::Consumer,
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
            Role::SupplierAdmin => "supplier_admin",
            Role::SupplierStaff => "supplier_staff",
            Role::Consumer => "consumer",
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
            Role::SupplierAdmin | Role::SupplierStaff => OrgKind::Supplier,
            Role::Consumer => OrgKind::Consumer,
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
    /// Issue Chuo-Sogo's own invoices and credit notes, maintain the issuer profile.
    ManageBilling,
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
    /// Record the customer's own purchase order for an approved recommendation.
    RecordOrders,
    /// Confirm goods received.
    ReceiveGoods,
    ViewOrders,
    ViewFinance,
    ManageInvoices,
    ViewReports,
    // Internal work performed on a customer org (requires a service assignment).
    OperateRequests,
    VerifyProducts,
    ManageSuppliers,
    /// Maintain Chuo-Sogo's product identity records (platform-level catalogue).
    ManageProducts,
    ManageRfqs,
    RecordQuotations,
    AssignWork,
    ViewInternalNotes,
    /// Supplier: create, edit and submit the company's own product listings.
    ManageListings,
    /// Chuo-Sogo: approve, reject or retire submitted listings.
    ModerateListings,
    /// Browse published listings.
    ViewCatalogue,
    /// Consumer: shop and order for oneself.
    Shop,
    /// Chuo-Sogo: run the consumer shop (orders, payment, shipping, shop rules).
    ManageShop,
    /// Import, view and remove the organisation's past-purchase records and buying criteria.
    ManagePurchaseHistory,
    /// See the organisation's agent runs, exceptions and agent settings.
    ViewAgents,
    /// Turn agents on or off, set their budget and mail rule, start a run by hand.
    ManageAgents,
    /// Approve, edit or reject what an agent proposes (the action's own permission is
    /// checked again when it is executed).
    DecideAgentExceptions,
}

impl Permission {
    pub const fn code(self) -> &'static str {
        match self {
            Permission::ManageOrganisations => "manage_organisations",
            Permission::ManageInternalUsers => "manage_internal_users",
            Permission::ManageServiceAssignments => "manage_service_assignments",
            Permission::ViewSystemHealth => "view_system_health",
            Permission::ManageBilling => "manage_billing",
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
            Permission::RecordOrders => "record_orders",
            Permission::ReceiveGoods => "receive_goods",
            Permission::ViewOrders => "view_orders",
            Permission::ViewFinance => "view_finance",
            Permission::ManageInvoices => "manage_invoices",
            Permission::ViewReports => "view_reports",
            Permission::OperateRequests => "operate_requests",
            Permission::VerifyProducts => "verify_products",
            Permission::ManageSuppliers => "manage_suppliers",
            Permission::ManageProducts => "manage_products",
            Permission::ManageRfqs => "manage_rfqs",
            Permission::RecordQuotations => "record_quotations",
            Permission::AssignWork => "assign_work",
            Permission::ViewInternalNotes => "view_internal_notes",
            Permission::ManageListings => "manage_listings",
            Permission::ModerateListings => "moderate_listings",
            Permission::ViewCatalogue => "view_catalogue",
            Permission::Shop => "shop",
            Permission::ManageShop => "manage_shop",
            Permission::ManagePurchaseHistory => "manage_purchase_history",
            Permission::ViewAgents => "view_agents",
            Permission::ManageAgents => "manage_agents",
            Permission::DecideAgentExceptions => "decide_agent_exceptions",
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
            ManageBilling,
            ModerateListings,
            ManageShop,
            ViewCatalogue,
            ViewAudit,
            ViewReports,
        ],
        Role::ProcurementAdmin => &[
            ManageServiceAssignments,
            ManageSuppliers,
            ManageProducts,
            ModerateListings,
            ManageShop,
            ViewCatalogue,
            ViewReports,
            ViewAudit,
        ],
        Role::ProcurementOperator => &[ManageSuppliers, ManageProducts, ViewCatalogue],
        Role::CustomerAdmin => &[
            ManageOrgUsers,
            ManageOrgSettings,
            ManageApprovalPolicy,
            ApproveSuppliers,
            ManagePurchaseHistory,
            ViewAgents,
            ManageAgents,
            DecideAgentExceptions,
            ViewAudit,
            SubmitRequest,
            ViewOwnRequests,
            ViewAllRequests,
            RecordOrders,
            ReceiveGoods,
            ViewOrders,
            ViewReports,
            ViewCatalogue,
        ],
        Role::ProcurementManager => &[
            SubmitRequest,
            ViewOwnRequests,
            ViewAllRequests,
            ReviewQuotations,
            ApproveRecommendation,
            ApproveSuppliers,
            ManagePurchaseHistory,
            ViewAgents,
            DecideAgentExceptions,
            RecordOrders,
            ReceiveGoods,
            ViewOrders,
            ViewReports,
            ViewCatalogue,
        ],
        Role::Requester => &[SubmitRequest, ViewOwnRequests, ReceiveGoods, ViewCatalogue],
        Role::Finance => &[
            ViewAllRequests,
            ViewOrders,
            ViewFinance,
            ManageInvoices,
            ViewReports,
            ViewAgents,
            DecideAgentExceptions,
        ],
        Role::SupplierAdmin => &[ManageOrgUsers, ViewAudit, ManageListings],
        Role::SupplierStaff => &[ManageListings],
        Role::Consumer => &[Shop],
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
            ManagePurchaseHistory,
            ManageSuppliers,
            ManageRfqs,
            RecordQuotations,
            AssignWork,
            ViewInternalNotes,
            ViewOrders,
            ViewReports,
            ViewAgents,
            DecideAgentExceptions,
        ],
        Role::ProcurementOperator => &[
            ViewAllRequests,
            OperateRequests,
            VerifyProducts,
            ManagePurchaseHistory,
            ManageRfqs,
            RecordQuotations,
            ViewInternalNotes,
            ViewOrders,
            ViewAgents,
            DecideAgentExceptions,
        ],
        Role::CustomerAdmin
        | Role::ProcurementManager
        | Role::Requester
        | Role::Finance
        | Role::SupplierAdmin
        | Role::SupplierStaff
        | Role::Consumer => &[],
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
    fn product_catalogue_is_staff_only() {
        for r in Role::ALL {
            let expected = matches!(r, Role::ProcurementAdmin | Role::ProcurementOperator);
            assert_eq!(
                allows(Access::Member(r), Permission::ManageProducts),
                expected,
                "{r}"
            );
            assert!(
                !allows(Access::Assigned(r), Permission::ManageProducts),
                "{r}"
            );
        }
    }

    #[test]
    fn orders_are_customer_decisions() {
        for r in Role::ALL {
            assert!(
                !allows(Access::Assigned(r), Permission::RecordOrders),
                "{r}"
            );
            assert!(
                !allows(Access::Assigned(r), Permission::ManageInvoices),
                "{r}"
            );
        }
        assert!(allows(
            Access::Member(Role::Requester),
            Permission::ReceiveGoods
        ));
        assert!(!allows(
            Access::Member(Role::Requester),
            Permission::RecordOrders
        ));
        assert!(allows(
            Access::Member(Role::Finance),
            Permission::ManageInvoices
        ));
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

    #[test]
    fn suppliers_only_manage_their_listings() {
        for r in [Role::SupplierAdmin, Role::SupplierStaff] {
            assert_eq!(r.org_kind(), OrgKind::Supplier);
            assert!(!r.is_internal());
            for p in [
                Permission::SubmitRequest,
                Permission::ViewAllRequests,
                Permission::ApproveRecommendation,
                Permission::ModerateListings,
                Permission::ViewCatalogue,
                Permission::ViewFinance,
            ] {
                assert!(!allows(Access::Member(r), p), "{r} {p:?}");
            }
            assert!(allows(Access::Member(r), Permission::ManageListings));
        }
        // Moderation is Chuo-Sogo's; listing is the supplier's.
        assert!(allows(
            Access::Member(Role::ProcurementAdmin),
            Permission::ModerateListings
        ));
        assert!(!allows(
            Access::Member(Role::ProcurementOperator),
            Permission::ModerateListings
        ));
        assert!(!allows(
            Access::Member(Role::PlatformSuperAdmin),
            Permission::ManageListings
        ));
    }

    #[test]
    fn consumers_only_shop_for_themselves() {
        let c = Access::Member(Role::Consumer);
        assert_eq!(Role::Consumer.org_kind(), OrgKind::Consumer);
        assert!(allows(c, Permission::Shop));
        for p in [
            Permission::ViewCatalogue,
            Permission::SubmitRequest,
            Permission::ManageListings,
            Permission::ManageOrgUsers,
            Permission::ManageShop,
            Permission::ViewAudit,
        ] {
            assert!(!allows(c, p), "{p:?}");
        }
        assert!(allows(
            Access::Member(Role::PlatformSuperAdmin),
            Permission::ManageShop
        ));
        assert!(!allows(
            Access::Member(Role::ProcurementOperator),
            Permission::ManageShop
        ));
    }

    #[test]
    fn purchase_history_is_kept_by_buyers_and_the_assigned_service_team() {
        let m = Permission::ManagePurchaseHistory;
        for r in [Role::CustomerAdmin, Role::ProcurementManager] {
            assert!(allows(Access::Member(r), m), "{r}");
        }
        for r in [Role::ProcurementAdmin, Role::ProcurementOperator] {
            assert!(allows(Access::Assigned(r), m), "{r}");
            // Never across all customers: only inside an assigned one.
            assert!(!allows(Access::Member(r), m), "{r}");
        }
        for r in [
            Role::Requester,
            Role::Finance,
            Role::SupplierAdmin,
            Role::Consumer,
        ] {
            assert!(!allows(Access::Member(r), m), "{r}");
        }
        assert!(!allows(Access::Assigned(Role::PlatformSuperAdmin), m));
    }

    #[test]
    fn agents_are_set_up_by_the_customer_and_decided_by_buyers_and_staff() {
        for r in Role::ALL {
            let member = Access::Member(r);
            assert_eq!(
                allows(member, Permission::ManageAgents),
                r == Role::CustomerAdmin,
                "{r}"
            );
            // Staff never change a customer's agent settings.
            assert!(
                !allows(Access::Assigned(r), Permission::ManageAgents),
                "{r}"
            );
            let decides = matches!(
                r,
                Role::CustomerAdmin | Role::ProcurementManager | Role::Finance
            );
            assert_eq!(
                allows(member, Permission::DecideAgentExceptions),
                decides,
                "{r}"
            );
            assert_eq!(allows(member, Permission::ViewAgents), decides, "{r}");
        }
        for r in [Role::ProcurementAdmin, Role::ProcurementOperator] {
            assert!(allows(
                Access::Assigned(r),
                Permission::DecideAgentExceptions
            ));
            assert!(allows(Access::Assigned(r), Permission::ViewAgents));
            assert!(!allows(Access::Member(r), Permission::ViewAgents));
        }
        assert!(!allows(
            Access::Member(Role::Requester),
            Permission::ViewAgents
        ));
    }
}
