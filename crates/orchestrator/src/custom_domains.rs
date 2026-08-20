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
