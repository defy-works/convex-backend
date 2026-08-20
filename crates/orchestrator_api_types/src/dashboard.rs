//! Types served under `/api/dashboard/*`, mirroring
//! `npm-packages/dashboard/dashboard-management-openapi.json`.

use serde::{
    Deserialize,
    Serialize,
};

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MemberResponse {
    pub id: u64,
    pub email: String,
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MemberDataResponse {
    pub member: MemberResponse,
    pub teams: Vec<TeamResponse>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProfileEmailResponse {
    pub id: u64,
    pub email: String,
    pub is_verified: bool,
    pub is_primary: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProfileEmailArgs {
    pub email: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateProfileNameArgs {
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TeamResponse {
    pub id: u64,
    pub name: String,
    pub slug: String,
    pub creator: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateTeamArgs {
    pub name: String,
    #[serde(default)]
    pub slug: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateTeamArgs {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub slug: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TeamMember {
    pub id: u64,
    pub email: String,
    pub name: Option<String>,
    pub role: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RemoveMemberArgs {
    pub member_id: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateMemberRoleArgs {
    pub member_id: u64,
    pub role: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct InvitationArgs {
    pub email: String,
    pub role: String,
}

/// Body of `teams/{team_id}/invites/cancel`. Distinct from `RemoveMemberArgs`,
/// which this used to borrow — that made the wire field for an invitation id
/// literally `memberId`.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CancelInvitationArgs {
    pub invitation_id: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct InvitationResponse {
    pub id: u64,
    pub email: String,
    pub role: String,
    pub code: String,
    pub created_at: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProjectResponse {
    pub id: u64,
    pub team_id: u64,
    pub name: String,
    pub slug: String,
    pub is_demo: bool,
    pub creation_time: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateProjectArgs {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub slug: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DeploymentResponse {
    pub id: u64,
    pub project_id: u64,
    pub name: String,
    pub deployment_type: String,
    pub deployment_class: String,
    pub url: String,
    pub site_url: String,
    pub state: String,
    pub creation_time: f64,
    pub region: Option<String>,
    pub preview_identifier: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct GetDeploymentAuthDashboardResponse {
    pub admin_key: String,
    /// The deployment's canonical origin — what its apps talk to, and what the
    /// dashboard displays. May be a custom domain the operator chose.
    pub url: String,
    /// The origin the dashboard should actually *connect* over.
    ///
    /// Always the orchestrator-derived `<name>.<router_host>` hostname, never
    /// a canonical override. A canonical URL is about where an operator's own
    /// app reaches the backend; if it is misconfigured — pointed at a CDN that
    /// intercepts requests, DNS not propagated, TLS wrong — that must not also
    /// cost them the admin console, which is the tool they need in order to
    /// fix it.
    pub console_url: String,
}

/// Body of `POST /api/dashboard/deployments/register` — used by operators in
/// `--provisioner external` mode to tell the orchestrator about a backend
/// they pre-started. The orchestrator stores the URL + an admin-key hash so
/// the dashboard and CLI can look it up later.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RegisterDeploymentArgs {
    pub deployment_name: String,
    pub project_id: u64,
    /// `prod`, `dev`, or `preview`.
    pub deployment_type: String,
    pub url: String,
    pub site_url: String,
    /// Full admin key the operator generated when starting their backend.
    /// Stored hashed; only the last few characters are retained in the clear
    /// (as `keySuffix`) for UI display.
    pub admin_key: String,
    #[serde(default)]
    pub region: Option<String>,
    /// Required for `preview` deployments — matches the preview branch / id.
    #[serde(default)]
    pub preview_identifier: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OptIn {
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct GetOptInsResponse {
    pub opt_ins_to_accept: Vec<OptIn>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AccessTokenResponse {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub creation_time: f64,
    pub key_suffix: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateAccessTokenArgs {
    pub name: String,
    #[serde(default)]
    pub expires_at: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateAccessTokenResponse {
    pub access_token: String,
    pub id: String,
    pub name: String,
    pub creation_time: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AuditLogEvent {
    pub id: u64,
    pub team_id: u64,
    pub member_id: Option<u64>,
    pub action: String,
    pub metadata: serde_json::Value,
    pub creation_time: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AuditLogPage {
    pub events: Vec<AuditLogEvent>,
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentVariable {
    pub name: String,
    pub value: String,
    pub deployment_types: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListEnvironmentVariables {
    pub variables: Vec<EnvironmentVariable>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateDefaultEnvVarsArgs {
    pub variables: Vec<EnvironmentVariable>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DeviceAuthorizeArgs {
    pub device_name: String,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub bootstrap_token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DeviceAuthorizeResponse {
    pub access_token: String,
    pub member_id: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CustomDomain {
    pub id: i64,
    pub deployment_id: i64,
    pub domain: String,
    /// `pending` -> `issuing` -> `active`, or `failed`. Only ever set from an
    /// observed outcome: `active` means an HTTPS request to the domain
    /// actually succeeded, not that issuance was requested.
    pub cert_state: String,
    pub created_at: i64,
    /// `api` — the Convex API / database — or `site` for HTTP actions.
    pub kind: String,
    /// `acme` when the orchestrator issues and renews the certificate,
    /// `upstream` when something in front (Cloudflare, another proxy) already
    /// terminates TLS. Upstream domains never enter issuance and are skipped
    /// by the renewal sweep, so their `certState` reflects reachability only.
    pub tls_mode: String,
    /// Verbatim reason the last issuance failed, if it did.
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateCustomDomainArgs {
    pub domain: String,
    /// `api` (default) or `site`. Chosen per domain so one deployment can
    /// front its database and its HTTP actions on different hostnames.
    #[serde(default)]
    pub kind: Option<String>,
    /// `acme` (default) or `upstream`. Pick `upstream` when the hostname is
    /// already fronted by something that terminates TLS, so no certificate is
    /// ordered for it.
    #[serde(default)]
    pub tls_mode: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListCustomDomains {
    pub domains: Vec<CustomDomain>,
    /// Hostname the operator should CNAME/A-record the custom domain at.
    /// Empty when the orchestrator has no public router host configured.
    pub target_host: String,
    /// False when the orchestrator has no Traefik dynamic directory wired up,
    /// in which case adding a domain would record a row that never routes.
    pub routing_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CustomDomainArgs {
    pub domain: String,
}


/// What a deployment advertises about itself, and what the operator wants it
/// to advertise. These become `CONVEX_CLOUD_ORIGIN` / `CONVEX_SITE_ORIGIN` on
/// the backend container, which is where `CONVEX_CLOUD_URL` /
/// `CONVEX_SITE_URL` and every generated HTTP action or auth callback URL
/// come from.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CanonicalUrls {
    /// What the *running* container was given.
    pub current_url: String,
    pub current_site_url: String,
    /// The operator's choice. Null means "use the derived default".
    pub desired_url: Option<String>,
    pub desired_site_url: Option<String>,
    /// The derived `<name>.<router_host>` forms. Always routed, whether or
    /// not an override is in effect.
    pub default_url: String,
    pub default_site_url: String,
    /// True when the desired origins differ from what the running container
    /// has — i.e. a restart is needed before the change takes effect.
    pub restart_pending: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SetCanonicalUrlsArgs {
    /// Null clears the override and returns the deployment to its derived
    /// hostname. Must otherwise be one of the deployment's attached custom
    /// domains of the matching kind — an origin that isn't routed here would
    /// only break the deployment.
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub site_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct VerifyCustomDomainResponse {
    pub domain: String,
    pub cert_state: String,
    /// Why the probe failed, when it did. Surfaced verbatim in the dashboard
    /// because the causes (DNS not pointed here, ACME rate limit) are things
    /// only the operator can fix.
    pub error: Option<String>,
}
