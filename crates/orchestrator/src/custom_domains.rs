//! Custom domain routing and certificate delivery.
//!
//! Spawned backends get their Traefik routers from docker labels
//! (`provisioner::docker`), but labels are fixed when the container is
//! created — a domain added later would never reach it. So custom domains go
//! through Traefik's *file* provider instead: the orchestrator renders every
//! domain into one YAML file that Traefik watches and hot-reloads.
//!
//! Certificates are issued by the orchestrator (see [`crate::acme`]) rather
//! than by Traefik, because Traefik's `certificatesResolvers` are static
//! configuration and could never be driven from the dashboard. We write the
//! PEMs next to the config and reference them under `tls.certificates`,
//! which the file provider *does* hot-reload. The upshot is that custom
//! domains need no static Traefik configuration at all beyond pointing the
//! file provider at this directory — no restarts, no editing compose over
//! SSH.
//!
//! The file is always rewritten in full from the database rather than
//! patched, so a crash mid-update can't leave a half-applied routing table:
//! whatever is in Postgres is the truth, and the next write reconciles.

use std::{
    collections::BTreeSet,
    net::IpAddr,
    path::{
        Path,
        PathBuf,
    },
};

use anyhow::Context;

use crate::{
    state::OrchestratorState,
    storage::{
        CustomDomainRoute,
        StoredCertificate,
    },
};

/// Filename written into the Traefik dynamic directory. Stable so each
/// rewrite replaces the previous routing table wholesale.
const CONFIG_FILENAME: &str = "custom-domains.yml";
/// Config lives in its own subdirectory, and Traefik's file provider watches
/// *that* rather than the volume root. The provider parses every file in the
/// directory it's pointed at, so keeping PEMs out of it is what stops Traefik
/// from trying to read a certificate as dynamic configuration.
const CONFIG_DIRNAME: &str = "conf";
/// Sibling subdirectory holding the PEMs referenced by the config above.
const CERT_DIRNAME: &str = "certs";

/// Ports the backend serves the Convex API and HTTP actions on. Same values
/// the docker-label routers use.
const BACKEND_API_PORT: u16 = 3210;
const BACKEND_SITE_PORT: u16 = 3211;

/// Rejects anything that isn't a plausible public hostname before it reaches
/// the routing table. A domain flows into a Traefik `Host()` rule, so a value
/// containing backticks or newlines could otherwise break out of the rule and
/// rewrite unrelated routing.
pub fn validate_domain(domain: &str) -> anyhow::Result<String> {
    let normalized = domain.trim().trim_end_matches('.').to_ascii_lowercase();

    anyhow::ensure!(!normalized.is_empty(), "domain must not be empty");
    anyhow::ensure!(
        normalized.len() <= 253,
        "domain must be 253 characters or fewer"
    );
    anyhow::ensure!(
        normalized.contains('.'),
        "domain must be fully qualified (e.g. api.example.com)"
    );
    anyhow::ensure!(
        !normalized.starts_with('-') && !normalized.starts_with('.'),
        "domain must not start with '-' or '.'"
    );
    // Wildcards can only be validated over DNS-01, which is not supported
    // (see `crate::acme`), so reject them here rather than letting issuance
    // fail a minute later with something less obvious.
    anyhow::ensure!(
        !normalized.contains('*'),
        "wildcard domains are not supported; add each hostname individually"
    );

    for label in normalized.split('.') {
        anyhow::ensure!(!label.is_empty(), "domain must not contain empty labels");
        anyhow::ensure!(
            label.len() <= 63,
            "each domain label must be 63 characters or fewer"
        );
        anyhow::ensure!(
            label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'),
            "domain labels may only contain letters, digits, and '-'"
        );
        anyhow::ensure!(
            !label.starts_with('-') && !label.ends_with('-'),
            "domain labels must not start or end with '-'"
        );
    }

    Ok(normalized)
}

/// Traefik router/service names must be unique and stable per domain. Dots
/// and other separators are collapsed so the name stays a single YAML key.
fn router_key(domain: &str) -> String {
    let slug: String = domain
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    format!("convex-custom-{slug}")
}

/// Filename (not path) of the PEM pair for a domain.
fn cert_stem(domain: &str) -> String {
    domain
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// Where the orchestrator serves ACME HTTP-01 challenge responses. Traefik
/// routes this path for every custom domain, on the plain `web` entrypoint,
/// so a domain can be validated *before* it has a certificate.
const ACME_CHALLENGE_PATH: &str = "/.well-known/acme-challenge/";

pub struct RenderInput<'a> {
    pub routes: &'a [CustomDomainRoute],
    pub certificates: &'a [StoredCertificate],
    pub container_prefix: &'a str,
    /// Host:port Traefik uses to reach the orchestrator for challenge
    /// responses, e.g. `orchestrator:8050`.
    pub orchestrator_upstream: &'a str,
    /// Directory the PEMs live in, as Traefik sees it.
    pub cert_dir: &'a str,
}

/// Renders the dynamic config. Kept pure (no I/O) so the exact YAML Traefik
/// will consume can be asserted in tests.
pub fn render_config(input: RenderInput<'_>) -> String {
    let mut routers = String::new();
    let mut services = String::new();
    let mut certificates = String::new();

    // `domain` is UNIQUE in the schema, but guard anyway: a duplicate key
    // would silently drop a router when Traefik parses the YAML.
    let mut seen = BTreeSet::new();

    // One challenge router shared by all custom domains. It matches the ACME
    // path on *any* host on the plain HTTP entrypoint; the ACME server only
    // ever requests it for domains we asked it to validate.
    if !input.routes.is_empty() {
        // Listens on *both* entrypoints on purpose. The `web` entrypoint has
        // a global http->https redirection whose internal router outranks
        // anything we can declare here, so a challenge request gets bounced
        // to `websecure` — where the domain has no valid certificate yet.
        // That's fine: Let's Encrypt follows redirects during HTTP-01 and
        // deliberately does not validate the certificate on the target. But
        // it only works if the challenge path is also routed on websecure,
        // otherwise the redirected request lands on the backend and 404s.
        routers.push_str(&format!(
            "    convex-acme-challenge:\n      rule: \"PathPrefix(`{ACME_CHALLENGE_PATH}`)\"\n     \
             \x20priority: 9000\n      entryPoints:\n        - web\n        - websecure\n      \
             service: convex-acme-challenge\n      tls: {{}}\n"
        ));
        services.push_str(&format!(
            "    convex-acme-challenge:\n      loadBalancer:\n        servers:\n          - url: \
             \"http://{}\"\n",
            input.orchestrator_upstream
        ));
    }

    for route in input.routes {
        if !seen.insert(route.domain.as_str()) {
            continue;
        }
        let key = router_key(&route.domain);
        let upstream = format!("{}{}", input.container_prefix, route.deployment_name);

        routers.push_str(&format!(
            "    {key}:\n      rule: \"Host(`{domain}`)\"\n      priority: 100\n      \
             entryPoints:\n        - websecure\n      service: {key}\n      tls: {{}}\n",
            key = key,
            domain = route.domain,
        ));
        services.push_str(&format!(
            "    {key}:\n      loadBalancer:\n        servers:\n          - url: \
             \"http://{upstream}:{port}\"\n",
            key = key,
            upstream = upstream,
            port = port_for_kind(&route.kind),
        ));
    }

    // Only reference PEMs that actually exist on disk. A domain without a
    // certificate still gets a router — Traefik serves its default cert and
    // the browser warns — which is strictly better than dropping the route
    // and is visible in the dashboard as `pending`.
    for cert in input.certificates {
        let stem = cert_stem(&cert.domain);
        certificates.push_str(&format!(
            "    - certFile: \"{dir}/{stem}.crt\"\n      keyFile: \"{dir}/{stem}.key\"\n",
            dir = input.cert_dir,
            stem = stem,
        ));
    }

    let mut out = String::from("# Managed by convex-orchestrator. Do not edit by hand.\n");

    if routers.is_empty() {
        // Nothing but the comment. Traefik rejects empty `routers: {}` /
        // `services: {}` maps outright — "routers cannot be a standalone
        // element (type map[string]*dynamic.Router)" — which killed the whole
        // file on load, so a config that was *meant* to say "nothing
        // configured" instead broke the provider. A comment-only document
        // parses to an empty configuration, which is what we actually want.
    } else {
        out.push_str("http:\n  routers:\n");
        out.push_str(&routers);
        out.push_str("  services:\n");
        out.push_str(&services);
    }

    if !certificates.is_empty() {
        out.push_str("tls:\n  certificates:\n");
        out.push_str(&certificates);
    }

    out
}

/// Which backend port a custom domain fronts. `site` domains carry HTTP
/// actions (:3211); everything else is the Convex API / database (:3210).
pub const KIND_API: &str = "api";
pub const KIND_SITE: &str = "site";

pub fn validate_kind(kind: &str) -> anyhow::Result<String> {
    match kind {
        KIND_API | KIND_SITE => Ok(kind.to_string()),
        other => anyhow::bail!("unknown custom domain kind {other:?} (expected `api` or `site`)"),
    }
}

/// Who terminates TLS for a custom domain.
///
/// `acme` is the default: the orchestrator orders a certificate over HTTP-01
/// and renews it. `upstream` means something in front of Traefik — Cloudflare
/// in proxied mode, another reverse proxy, a load balancer — already presents
/// a certificate to the browser, so ordering one here would be pointless work
/// against the ACME rate limits.
///
/// An `upstream` domain still gets a Traefik router on `websecure`, served
/// with Traefik's default certificate. That satisfies Cloudflare's `Full`
/// mode, which does not validate the origin certificate. It does *not*
/// satisfy `Full (strict)` — use `acme` for that. Cloudflare's `Flexible`
/// mode (plain HTTP to the origin) is not supported either way: the `web`
/// entrypoint carries a global http->https redirect that outranks anything
/// declared here, so a custom-domain router on `web` would never be reached.
pub const TLS_MODE_ACME: &str = "acme";
pub const TLS_MODE_UPSTREAM: &str = "upstream";

/// `cert_state` values. `active` is only ever written by `probe_domain` after
/// the deployment identified itself over the hostname, so it is the one state
/// that proves the domain routes here — which is what gates making it
/// canonical.
pub const CERT_STATE_ACTIVE: &str = "active";
pub const CERT_STATE_PENDING: &str = "pending";

/// The bare hostname inside an origin, e.g. `https://api.example.com:8443/x`
/// -> `api.example.com`. Used to decide whether a canonical URL points at a
/// given custom domain; returns the input unchanged when it is already bare.
pub fn host_of(url: &str) -> &str {
    url.split("://")
        .nth(1)
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or(url)
        .split(':')
        .next()
        .unwrap_or(url)
}

pub fn validate_tls_mode(mode: &str) -> anyhow::Result<String> {
    match mode {
        TLS_MODE_ACME | TLS_MODE_UPSTREAM => Ok(mode.to_string()),
        other => anyhow::bail!(
            "unknown custom domain TLS mode {other:?} (expected `acme` or `upstream`)"
        ),
    }
}

fn port_for_kind(kind: &str) -> u16 {
    if kind == KIND_SITE {
        BACKEND_SITE_PORT
    } else {
        BACKEND_API_PORT
    }
}

fn config_path(dir: &Path) -> PathBuf {
    dir.join(CONFIG_DIRNAME).join(CONFIG_FILENAME)
}

/// Re-renders the whole custom-domain routing table from the database and
/// writes it, plus every stored certificate, into the Traefik dynamic
/// directory. No-op when the feature is disabled (no directory configured).
///
/// Writes go to a temp file and are then renamed, because Traefik watches the
/// directory and would otherwise happily load a truncated file.
pub async fn sync_traefik_config(state: &OrchestratorState) -> anyhow::Result<()> {
    let Some(dir) = state.config.traefik_dynamic_dir.clone() else {
        return Ok(());
    };

    let routes = state
        .storage
        .list_all_custom_domain_routes()
        .await
        .context("listing custom domains for Traefik config")?;
    let certificates = state
        .storage
        .list_certificates()
        .await
        .context("listing certificates for Traefik config")?;

    let conf_dir = dir.join(CONFIG_DIRNAME);
    std::fs::create_dir_all(&conf_dir)
        .with_context(|| format!("creating Traefik config dir {conf_dir:?}"))?;
    let cert_dir = dir.join(CERT_DIRNAME);
    std::fs::create_dir_all(&cert_dir)
        .with_context(|| format!("creating certificate dir {cert_dir:?}"))?;

    // Write PEMs before the config that references them, so Traefik never
    // reloads a config pointing at a file that isn't there yet.
    for cert in &certificates {
        let stem = cert_stem(&cert.domain);
        write_atomic(&cert_dir.join(format!("{stem}.crt")), &cert.cert_pem)?;
        write_atomic(&cert_dir.join(format!("{stem}.key")), &cert.key_pem)?;
    }

    let body = render_config(RenderInput {
        routes: &routes,
        certificates: &certificates,
        container_prefix: &state.config.backend_container_prefix,
        orchestrator_upstream: &state.config.orchestrator_upstream,
        // Traefik sees the same volume; the path is whatever it's mounted at
        // on its side.
        cert_dir: &format!("{}/{CERT_DIRNAME}", state.config.traefik_cert_dir),
    });

    write_atomic(&config_path(&dir), &body)?;

    tracing::info!(
        domains = routes.len(),
        certificates = certificates.len(),
        path = ?config_path(&dir),
        "wrote Traefik custom-domain config"
    );
    Ok(())
}

fn write_atomic(path: &Path, body: &str) -> anyhow::Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, body).with_context(|| format!("writing {tmp:?}"))?;
    std::fs::rename(&tmp, path).with_context(|| format!("renaming {tmp:?} -> {path:?}"))?;
    Ok(())
}

/// Probes the domain over HTTPS and confirms **this deployment** answers.
///
/// `/instance_name` is unauthenticated and returns the backend's own instance
/// name, so comparing the body is what separates "reaches this deployment"
/// from "reaches something". This used to treat any HTTP response as proof,
/// which made `active` meaningless: a CDN bot-protection challenge, a parked
/// domain, another tenant, or Traefik's own 404 on an unrouted host all
/// answer with a valid TLS handshake and some HTTP status. The Check button
/// reported `active` for every one of them.
///
/// Returns `("active", None)` only when the deployment identifies itself.
pub async fn probe_domain(
    domain: &str,
    expected_deployment: &str,
) -> (String, Option<String>) {
    let url = format!("https://{domain}/instance_name");
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
    {
        Ok(c) => c,
        Err(e) => return ("pending".to_string(), Some(e.to_string())),
    };

    let resp = match client.get(&url).send().await {
        Ok(r) => r,
        // No TLS handshake, DNS failure, timeout — nothing is being served
        // here for us yet.
        Err(e) => return ("pending".to_string(), Some(e.to_string())),
    };

    let status = resp.status().as_u16();
    let body = resp.text().await.unwrap_or_default();
    classify_probe_response(domain, expected_deployment, status, &body)
}

/// What a full verification learned about a domain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DomainDetection {
    /// `active` only when this deployment answered over the hostname.
    pub cert_state: String,
    /// The TLS mode we inferred. `None` means "not confident — leave the
    /// stored mode alone", which is the case when the domain neither points
    /// here nor answers as us.
    pub tls_mode: Option<String>,
    pub error: Option<String>,
}

/// Whether a resolved address is reachable from the public internet, and so
/// meaningful to compare a customer's DNS record against.
///
/// The orchestrator resolves its own router host from inside its container,
/// where that name frequently answers with the machine's private view —
/// docker bridge gateways (172.17.0.1), the VPC address (172.31.x.x), IPv6
/// link-local. A customer's CNAME resolves to the *public* address, so
/// comparing against the private view never intersects and rejects a
/// correctly-pointed domain.
fn is_public_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            !v4.is_private()
                && !v4.is_loopback()
                && !v4.is_link_local()
                && !v4.is_broadcast()
                && !v4.is_unspecified()
                // Documentation ranges (192.0.2/24, 198.51.100/24, 203.0.113/24)
                // are deliberately NOT excluded: they never appear in real DNS,
                // and excluding them only breaks tests that use them by
                // convention.
                // 100.64.0.0/10, carrier-grade NAT — also not addressable.
                && !(v4.octets()[0] == 100 && (64..128).contains(&v4.octets()[1]))
        },
        IpAddr::V6(v6) => {
            !v6.is_loopback()
                && !v6.is_unspecified()
                // fe80::/10 link-local and fc00::/7 unique-local.
                && !(v6.segments()[0] & 0xffc0 == 0xfe80)
                && !(v6.octets()[0] & 0xfe == 0xfc)
        },
    }
}

/// What the two resolutions say about where the domain points.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DnsVerdict {
    PointsHere,
    PointsElsewhere,
    /// We could not establish our own public address, so the comparison is
    /// meaningless and must not be used to reject anything.
    Unknown,
}

fn compare_resolutions(
    domain_ips: &Result<BTreeSet<IpAddr>, String>,
    router_ips: &Result<BTreeSet<IpAddr>, String>,
) -> DnsVerdict {
    let public = |set: &Result<BTreeSet<IpAddr>, String>| -> BTreeSet<IpAddr> {
        set.as_ref()
            .map(|s| s.iter().copied().filter(is_public_ip).collect())
            .unwrap_or_default()
    };
    let router = public(router_ips);
    if router.is_empty() {
        // Local dev (`localhost`), or a container whose view of its own
        // hostname is private. Either way we cannot tell direct from proxied.
        return DnsVerdict::Unknown;
    }
    let domain = public(domain_ips);
    if domain.is_empty() {
        // We know where we are and the domain isn't there — a record that
        // doesn't resolve is conclusively not pointing at us.
        return DnsVerdict::PointsElsewhere;
    }
    if domain.intersection(&router).next().is_some() {
        DnsVerdict::PointsHere
    } else {
        DnsVerdict::PointsElsewhere
    }
}

/// Resolve a hostname to its addresses. `Err` carries the resolver's message.
async fn resolve_ips(host: &str) -> Result<BTreeSet<IpAddr>, String> {
    // Port is irrelevant; `lookup_host` just needs one to form a socket addr.
    match tokio::net::lookup_host((host, 443u16)).await {
        Ok(addrs) => Ok(addrs.map(|a| a.ip()).collect()),
        Err(e) => Err(e.to_string()),
    }
}

/// Verify a custom domain end to end, and infer how its TLS is terminated.
///
/// Two independent signals, because neither alone is sufficient:
///
/// - **Where DNS points.** If the domain resolves to the same addresses as the
///   orchestrator's own router host, traffic arrives here directly, so nothing
///   else can be terminating TLS and the certificate has to be ours (`acme`).
/// - **Whether the deployment answers.** An HTTPS request to the domain must
///   come back as this deployment. A proxy in front can satisfy this while DNS
///   points at the proxy — that is exactly what `upstream` means.
///
/// A domain that neither resolves here nor answers as us is not verified, and
/// the error names both resolutions so the operator can see the mismatch.
pub async fn detect_domain(
    domain: &str,
    router_host: &str,
    expected_deployment: &str,
) -> DomainDetection {
    let domain_ips = resolve_ips(domain).await;
    let router_ips = resolve_ips(router_host).await;
    let probe = probe_body(domain).await;
    classify_detection(
        domain,
        router_host,
        expected_deployment,
        &domain_ips,
        &router_ips,
        &probe,
    )
}

/// `GET https://<domain>/instance_name`, returning the trimmed body or the
/// transport error.
async fn probe_body(domain: &str) -> Result<String, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client
        .get(format!("https://{domain}/instance_name"))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = resp.status().as_u16();
    let body = resp.text().await.unwrap_or_default();
    let trimmed = body.trim().to_string();
    if trimmed.is_empty() {
        return Err(format!("HTTP {status} with an empty body"));
    }
    Ok(trimmed)
}

fn render_ips(ips: &Result<BTreeSet<IpAddr>, String>) -> String {
    match ips {
        Err(e) => format!("does not resolve ({e})"),
        Ok(set) if set.is_empty() => "does not resolve".to_string(),
        Ok(set) => format!(
            "resolves to {}",
            set.iter()
                .take(4)
                .map(|ip| ip.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// The decision `detect_domain` makes from its three signals, split out so the
/// combinations are testable without DNS or a network.
fn classify_detection(
    domain: &str,
    router_host: &str,
    expected_deployment: &str,
    domain_ips: &Result<BTreeSet<IpAddr>, String>,
    router_ips: &Result<BTreeSet<IpAddr>, String>,
    probe: &Result<String, String>,
) -> DomainDetection {
    let dns = compare_resolutions(domain_ips, router_ips);
    let answered_as_us = probe.as_deref().map(|b| b == expected_deployment).unwrap_or(false);
    let points_here = dns == DnsVerdict::PointsHere;

    if answered_as_us {
        return DomainDetection {
            cert_state: CERT_STATE_ACTIVE.to_string(),
            // Reaching us without DNS pointing here means something in front
            // forwarded the request, and that something owns the certificate.
            // When the comparison was inconclusive we cannot tell the two
            // apart, so leave the stored mode alone rather than guessing.
            tls_mode: match dns {
                DnsVerdict::PointsHere => Some(TLS_MODE_ACME.to_string()),
                DnsVerdict::PointsElsewhere => Some(TLS_MODE_UPSTREAM.to_string()),
                DnsVerdict::Unknown => None,
            },
            error: None,
        };
    }

    if points_here {
        // DNS is right, so this is ours to serve — the domain just isn't
        // serving yet. A freshly added domain sits here until issuance lands.
        let detail = match probe {
            Ok(body) => format!(
                "got `{}`",
                body.chars().take(80).collect::<String>()
            ),
            Err(e) => e.clone(),
        };
        return DomainDetection {
            cert_state: CERT_STATE_PENDING.to_string(),
            tls_mode: Some(TLS_MODE_ACME.to_string()),
            error: Some(format!(
                "DNS for {domain} points at this orchestrator, but it is not \
                 serving {expected_deployment} yet: {detail}. This is normal \
                 until the certificate is issued."
            )),
        };
    }

    let detail = match probe {
        Ok(body) => format!(
            "it answered with `{}`",
            body.chars().take(80).collect::<String>()
        ),
        Err(e) => format!("the request failed: {e}"),
    };
    let error = match dns {
        DnsVerdict::PointsElsewhere => format!(
            "DNS for {domain} {}, which is not this orchestrator ({router_host} {}), \
             and requests to it do not reach {expected_deployment} — {detail}. Point \
             the record at {router_host}; if it is behind a CDN, switch that record \
             to DNS-only so requests arrive here directly.",
            render_ips(domain_ips),
            render_ips(router_ips),
        ),
        // Don't blame DNS for something we could not check. This is the
        // container-view case: the orchestrator's own hostname resolved only
        // to private addresses, so there was nothing public to compare.
        DnsVerdict::Unknown => format!(
            "Requests to {domain} do not reach {expected_deployment} — {detail}. \
             Point the record at {router_host}; if it is behind a CDN, switch that \
             record to DNS-only so requests arrive here directly. (This orchestrator \
             could not determine its own public address, so DNS was not compared: \
             {router_host} {} from inside the container.)",
            render_ips(router_ips),
        ),
        // Unreachable: PointsHere is handled above.
        DnsVerdict::PointsHere => unreachable!("handled above"),
    };
    DomainDetection {
        cert_state: CERT_STATE_PENDING.to_string(),
        // Neither signal is conclusive; don't overwrite what is stored.
        tls_mode: None,
        error: Some(error),
    }
}

/// The decision `probe_domain` makes once a response has arrived, split out so
/// it can be tested without a network.
fn classify_probe_response(
    domain: &str,
    expected_deployment: &str,
    status: u16,
    body: &str,
) -> (String, Option<String>) {
    let answered = body.trim();
    if answered == expected_deployment {
        return (CERT_STATE_ACTIVE.to_string(), None);
    }
    // A response arrived but it is not this deployment. Quote a bounded slice
    // of what did answer — an HTML challenge page is the usual culprit and
    // recognisable from its first line.
    let excerpt: String = answered.chars().take(80).collect();
    (
        CERT_STATE_PENDING.to_string(),
        Some(format!(
            "{domain} answered with HTTP {status} but did not identify as \
             `{expected_deployment}`; got `{excerpt}`. Something in front of \
             the backend — a CDN challenge, another service on this hostname, \
             or a router pointing elsewhere — is intercepting requests."
        )),
    )
}

#[cfg(test)]
mod detection_tests {
    use super::*;

    fn ips(list: &[&str]) -> Result<BTreeSet<IpAddr>, String> {
        Ok(list.iter().map(|s| s.parse().expect("ip")).collect())
    }

    const ORCH: &[&str] = &["203.0.113.10"];
    // Cloudflare-style: the record resolves to the proxy, not to us.
    const CDN: &[&str] = &["104.21.5.5"];

    #[test]
    fn dns_here_and_serving_is_active_and_ours_to_issue() {
        let d = classify_detection(
            "api.example.com",
            "convex.example.com",
            "kind-panda-859",
            &ips(ORCH),
            &ips(ORCH),
            &Ok("kind-panda-859".into()),
        );
        assert_eq!(d.cert_state, CERT_STATE_ACTIVE);
        assert_eq!(d.tls_mode.as_deref(), Some(TLS_MODE_ACME));
        assert_eq!(d.error, None);
    }

    #[test]
    fn a_proxy_that_forwards_correctly_is_active_and_upstream() {
        // Reaching us without DNS pointing here means something in front
        // terminated TLS and forwarded — that is what `upstream` means, and
        // it is now inferred rather than declared by the operator.
        let d = classify_detection(
            "api.example.com",
            "convex.example.com",
            "kind-panda-859",
            &ips(CDN),
            &ips(ORCH),
            &Ok("kind-panda-859".into()),
        );
        assert_eq!(d.cert_state, CERT_STATE_ACTIVE);
        assert_eq!(d.tls_mode.as_deref(), Some(TLS_MODE_UPSTREAM));
    }

    #[test]
    fn dns_here_but_not_serving_yet_is_pending_and_ours() {
        // The state a freshly added domain sits in until issuance lands.
        let d = classify_detection(
            "api.example.com",
            "convex.example.com",
            "kind-panda-859",
            &ips(ORCH),
            &ips(ORCH),
            &Err("connection refused".into()),
        );
        assert_eq!(d.cert_state, CERT_STATE_PENDING);
        assert_eq!(d.tls_mode.as_deref(), Some(TLS_MODE_ACME));
        assert!(d.error.expect("error").contains("until the certificate is issued"));
    }

    #[test]
    fn pointing_elsewhere_and_not_reaching_us_is_rejected_with_both_resolutions() {
        // The Cloudflare-challenge case: something answers, but not as us.
        let d = classify_detection(
            "backend.prime-reserve.com",
            "convex.example.com",
            "kind-panda-859",
            &ips(CDN),
            &ips(ORCH),
            &Ok("<!DOCTYPE html><title>Just a moment...</title>".into()),
        );
        assert_eq!(d.cert_state, CERT_STATE_PENDING);
        // Not confident either way — must not clobber the stored mode.
        assert_eq!(d.tls_mode, None);
        let err = d.error.expect("error");
        assert!(err.contains("104.21.5.5"), "names where it does point: {err}");
        assert!(err.contains("203.0.113.10"), "names where it should point: {err}");
        assert!(err.contains("DNS-only"), "tells them how to fix it: {err}");
    }

    #[test]
    fn a_domain_that_does_not_resolve_says_so() {
        let d = classify_detection(
            "typo.example.com",
            "convex.example.com",
            "kind-panda-859",
            &Err("no such host".into()),
            &ips(ORCH),
            &Err("dns error".into()),
        );
        assert_eq!(d.cert_state, CERT_STATE_PENDING);
        assert_eq!(d.tls_mode, None);
        assert!(d.error.expect("error").contains("does not resolve"));
    }

    // The orchestrator resolves its own router host from inside a container,
    // where that name often answers with the machine's private view. Observed
    // in production: defyhost.com -> 172.17.0.1 (docker0), 172.18.0.1,
    // 172.31.46.31 (VPC), fe80::… — none of which a customer's CNAME could
    // ever match, so a correctly-pointed domain was rejected.
    const CONTAINER_VIEW: &[&str] =
        &["172.17.0.1", "172.18.0.1", "172.31.46.31", "fe80::4f2:3bff:fe06:9111"];

    #[test]
    fn a_private_view_of_our_own_host_is_not_used_to_reject() {
        let d = classify_detection(
            "api.prime-reserve.com",
            "defyhost.com",
            "calm-lynx-792",
            &ips(&["203.0.113.99"]),
            &ips(CONTAINER_VIEW),
            &Ok("calm-lynx-792".into()),
        );
        // The deployment answered; that is what counts.
        assert_eq!(d.cert_state, CERT_STATE_ACTIVE);
        // But we could not tell direct from proxied, so don't guess a mode.
        assert_eq!(d.tls_mode, None);
    }

    #[test]
    fn an_unusable_self_resolution_does_not_claim_dns_is_wrong() {
        let d = classify_detection(
            "api.prime-reserve.com",
            "defyhost.com",
            "calm-lynx-792",
            &ips(&["104.21.25.228"]),
            &ips(CONTAINER_VIEW),
            &Ok("<!DOCTYPE html>".into()),
        );
        assert_eq!(d.cert_state, CERT_STATE_PENDING);
        let err = d.error.expect("error");
        assert!(
            err.contains("could not determine its own public address"),
            "must admit the comparison was skipped: {err}"
        );
        assert!(
            !err.contains("which is not this orchestrator"),
            "must not assert a mismatch it never established: {err}"
        );
    }

    #[test]
    fn a_public_self_resolution_still_detects_a_proxy() {
        // Regression guard: filtering private addresses must not weaken the
        // real comparison when the router host does resolve publicly.
        let d = classify_detection(
            "api.prime-reserve.com",
            "convex.example.com",
            "calm-lynx-792",
            &ips(&["104.21.25.228", "172.67.134.218"]),
            &ips(&["203.0.113.10", "172.17.0.1"]),
            &Ok("<!DOCTYPE html>".into()),
        );
        let err = d.error.expect("error");
        assert!(err.contains("which is not this orchestrator"), "{err}");
        assert!(err.contains("DNS-only"), "{err}");
    }

    #[test]
    fn an_unresolvable_router_host_falls_back_to_the_probe() {
        // `router_host` is `localhost` in local dev, so the IP comparison is
        // meaningless there. Reaching the deployment must still count.
        let d = classify_detection(
            "api.example.com",
            "localhost",
            "kind-panda-859",
            &ips(CDN),
            &Err("no address".into()),
            &Ok("kind-panda-859".into()),
        );
        assert_eq!(d.cert_state, CERT_STATE_ACTIVE);
    }
}

#[cfg(test)]
mod probe_tests {
    use super::*;

    // The bug this guards: the probe used to treat *any* HTTP response as
    // proof, so Check reported `active` for a CDN challenge page, a parked
    // domain, another tenant, or Traefik's own 404 on an unrouted host.

    #[test]
    fn the_deployment_identifying_itself_is_active() {
        let (state, err) = classify_probe_response(
            "api.example.com",
            "kind-panda-859",
            200,
            "kind-panda-859",
        );
        assert_eq!(state, CERT_STATE_ACTIVE);
        assert_eq!(err, None);
    }

    #[test]
    fn trailing_whitespace_still_counts_as_a_match() {
        // The endpoint returns a bare string; curl-style newlines must not
        // make a working domain look broken.
        let (state, _) = classify_probe_response(
            "api.example.com",
            "kind-panda-859",
            200,
            "kind-panda-859\n",
        );
        assert_eq!(state, CERT_STATE_ACTIVE);
    }

    #[test]
    fn a_cdn_challenge_page_is_not_active() {
        let (state, err) = classify_probe_response(
            "backend.prime-reserve.com",
            "kind-panda-859",
            403,
            "<!DOCTYPE html><html lang=\"en-US\"><head><title>Just a moment...</title>",
        );
        assert_eq!(state, CERT_STATE_PENDING);
        let err = err.expect("a mismatch must explain itself");
        assert!(err.contains("did not identify as"), "{err}");
        assert!(err.contains("Just a moment"), "excerpt must be quoted: {err}");
    }

    #[test]
    fn another_deployment_answering_is_not_active() {
        // Two deployments behind the same wildcard: routing sends the request
        // to the wrong container. Answering successfully is not enough.
        let (state, _) = classify_probe_response(
            "api.example.com",
            "kind-panda-859",
            200,
            "sunny-deer-163",
        );
        assert_eq!(state, CERT_STATE_PENDING);
    }

    #[test]
    fn an_empty_body_is_not_active() {
        let (state, _) =
            classify_probe_response("api.example.com", "kind-panda-859", 200, "");
        assert_eq!(state, CERT_STATE_PENDING);
    }

    #[test]
    fn a_long_html_body_is_truncated_in_the_error() {
        let body = "x".repeat(5000);
        let (_, err) =
            classify_probe_response("api.example.com", "kind-panda-859", 200, &body);
        // Bounded so a whole HTML page can't land in a DB column or the UI.
        assert!(err.expect("error").len() < 400);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(domain: &str, deployment: &str) -> CustomDomainRoute {
        CustomDomainRoute {
            domain: domain.to_string(),
            deployment_name: deployment.to_string(),
            kind: KIND_API.to_string(),
        }
    }

    fn site_route(domain: &str, deployment: &str) -> CustomDomainRoute {
        CustomDomainRoute {
            domain: domain.to_string(),
            deployment_name: deployment.to_string(),
            kind: KIND_SITE.to_string(),
        }
    }

    fn cert(domain: &str) -> StoredCertificate {
        StoredCertificate {
            domain: domain.to_string(),
            cert_pem: "cert".into(),
            key_pem: "key".into(),
            issued_at: 0,
            renew_after: 0,
        }
    }

    fn render(routes: &[CustomDomainRoute], certs: &[StoredCertificate]) -> String {
        render_config(RenderInput {
            routes,
            certificates: certs,
            container_prefix: "orchestrator-",
            orchestrator_upstream: "orchestrator:8050",
            cert_dir: "/dynamic/certs",
        })
    }

    #[test]
    fn normalizes_domains() {
        assert_eq!(
            validate_domain(" API.Example.COM. ").unwrap(),
            "api.example.com"
        );
    }

    #[test]
    fn rejects_domains_that_could_break_out_of_the_traefik_rule() {
        for bad in [
            "",
            "example",
            "ex`ample.com",
            "example.com`)||Host(`evil.com",
            "-lead.example.com",
            "trail-.example.com",
            "double..dot.com",
            "sub.*.example.com",
            "*.example.com",
        ] {
            assert!(
                validate_domain(bad).is_err(),
                "expected {bad:?} to be rejected"
            );
        }
    }

    #[test]
    fn rejects_wildcards() {
        // Only DNS-01 can validate a wildcard, and that is not supported.
        assert!(validate_domain("*.example.com").is_err());
    }

    #[test]
    fn renders_a_router_and_service_per_domain() {
        let config = render(&[route("api.example.com", "happy-otter-123")], &[]);
        assert!(config.contains("convex-custom-api-example-com:"));
        assert!(config.contains("rule: \"Host(`api.example.com`)\""));
        assert!(config.contains("url: \"http://orchestrator-happy-otter-123:3210\""));
    }

    #[test]
    fn site_domains_target_the_http_actions_port() {
        // The whole point of `kind`: an api domain and a site domain on the
        // same deployment must reach different ports.
        let config = render(
            &[
                route("api.example.com", "otter"),
                site_route("hooks.example.com", "otter"),
            ],
            &[],
        );
        assert!(config.contains("url: \"http://orchestrator-otter:3210\""));
        assert!(config.contains("url: \"http://orchestrator-otter:3211\""));
    }

    #[test]
    fn rejects_unknown_kinds() {
        assert_eq!(validate_kind("api").unwrap(), "api");
        assert_eq!(validate_kind("site").unwrap(), "site");
        assert!(validate_kind("database").is_err());
    }

    #[test]
    fn host_of_strips_scheme_port_and_path() {
        assert_eq!(host_of("https://api.example.com"), "api.example.com");
        assert_eq!(host_of("https://api.example.com/x/y"), "api.example.com");
        assert_eq!(host_of("http://api.example.com:8443"), "api.example.com");
        // Already bare, which is how domains are stored.
        assert_eq!(host_of("api.example.com"), "api.example.com");
    }

    #[test]
    fn rejects_unknown_tls_modes() {
        assert_eq!(validate_tls_mode("acme").unwrap(), "acme");
        assert_eq!(validate_tls_mode("upstream").unwrap(), "upstream");
        assert!(validate_tls_mode("cloudflare").is_err());
        assert!(validate_tls_mode("").is_err());
    }

    #[test]
    fn upstream_domains_still_get_a_router() {
        // The point of `upstream` is to skip *issuance*, not routing: traffic
        // still arrives at Traefik and has to reach the backend. Rendering is
        // deliberately identical to `acme` — the only difference is that no
        // certificate row ever exists, so Traefik serves its default cert,
        // which is what Cloudflare's `Full` mode expects.
        let rendered = render(&[route("proxied.example.com", "dep-1")], &[]);
        assert!(rendered.contains("Host(`proxied.example.com`)"));
        assert!(!rendered.contains("tls:\n  certificates:"));
    }

    #[test]
    fn never_references_a_traefik_cert_resolver() {
        // Cert resolvers are static Traefik config; relying on one would put
        // this feature back behind a restart. Certificates must come from
        // `tls.certificates` instead.
        let config = render(
            &[route("api.example.com", "one")],
            &[cert("api.example.com")],
        );
        assert!(!config.contains("certResolver"));
        assert!(config.contains("tls:\n  certificates:"));
        assert!(config.contains("certFile: \"/dynamic/certs/api-example-com.crt\""));
        assert!(config.contains("keyFile: \"/dynamic/certs/api-example-com.key\""));
    }

    #[test]
    fn routes_the_acme_challenge_path_on_both_entrypoints() {
        // HTTP-01 has to work before any certificate exists. The global
        // http->https redirect outranks anything the file provider can
        // declare, so the challenge must also be served on websecure — Let's
        // Encrypt follows the redirect and ignores the (missing) cert, but
        // only reaches us if that path is routed there too.
        let config = render(&[route("api.example.com", "one")], &[]);
        assert!(config.contains("PathPrefix(`/.well-known/acme-challenge/`)"));
        assert!(config.contains("        - web\n        - websecure\n"));
        assert!(config.contains("priority: 9000"));
        assert!(config.contains("url: \"http://orchestrator:8050\""));
    }

    #[test]
    fn a_domain_without_a_certificate_still_routes() {
        let config = render(&[route("api.example.com", "one")], &[]);
        assert!(config.contains("convex-custom-api-example-com:"));
        assert!(!config.contains("tls:\n  certificates:"));
    }

    #[test]
    fn emits_only_a_comment_when_there_are_no_domains() {
        // Verified against a live Traefik: `routers: {}` / `services: {}` is
        // rejected with "routers cannot be a standalone element", which takes
        // the entire file down. A comment-only document is accepted and
        // yields an empty configuration.
        let config = render(&[], &[]);
        assert!(!config.contains("routers"));
        assert!(!config.contains("services"));
        assert!(!config.contains("http:"));
        assert!(!config.contains("acme-challenge"));
        assert!(config.starts_with("# Managed by convex-orchestrator"));
        // Every non-empty line must be a comment, or Traefik will try to
        // interpret it as configuration.
        assert!(config
            .lines()
            .filter(|l| !l.trim().is_empty())
            .all(|l| l.trim_start().starts_with('#')));
    }

    #[test]
    fn skips_duplicate_domains() {
        let config = render(
            &[
                route("api.example.com", "one"),
                route("api.example.com", "two"),
            ],
            &[],
        );
        assert!(config.contains("orchestrator-one:3210"));
        assert!(!config.contains("orchestrator-two"));
    }
}
