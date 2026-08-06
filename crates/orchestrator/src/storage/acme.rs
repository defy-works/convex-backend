//! Storage for ACME state: the account key and issued certificates.
//!
//! `credentials` is returned as raw bytes; only `SecretSealer` can open it,
//! and nothing here ever hands a decrypted secret to an API response.

use super::Storage;
use crate::time::now_unix_ms;

#[derive(Debug, Clone)]
pub struct AcmeAccountRecord {
    pub account_url: String,
    pub credentials: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct StoredCertificate {
    pub domain: String,
    pub cert_pem: String,
    pub key_pem: String,
    pub issued_at: i64,
    pub renew_after: i64,
}

impl Storage {
    // ---------- ACME account ----------

    pub async fn get_acme_account(
        &self,
        directory_url: &str,
    ) -> anyhow::Result<Option<AcmeAccountRecord>> {
        let conn = self.pool().acquire().await?;
        let row = conn
            .client()
            .query_opt(
                "SELECT account_url, credentials FROM acme_accounts WHERE directory_url = $1",
                &[&directory_url],
            )
            .await?;
        Ok(row.map(|r| AcmeAccountRecord {
            account_url: r.get(0),
            credentials: r.get(1),
        }))
    }

    pub async fn upsert_acme_account(
        &self,
        directory_url: &str,
        account_url: &str,
        credentials: &[u8],
    ) -> anyhow::Result<()> {
        let conn = self.pool().acquire().await?;
        conn.client()
            .execute(
                "INSERT INTO acme_accounts (directory_url, account_url, credentials, created_at)
                 VALUES ($1, $2, $3, $4)
                 ON CONFLICT (directory_url) DO UPDATE
                   SET account_url = EXCLUDED.account_url,
                       credentials = EXCLUDED.credentials",
                &[
                    &directory_url,
                    &account_url,
                    &credentials.to_vec(),
                    &now_unix_ms(),
                ],
            )
            .await?;
        Ok(())
    }

    // ---------- Certificates ----------

    pub async fn upsert_certificate(
        &self,
        domain: &str,
        cert_pem: &str,
        key_pem: &str,
        issued_at: i64,
        renew_after: i64,
    ) -> anyhow::Result<()> {
        let conn = self.pool().acquire().await?;
        conn.client()
            .execute(
                "INSERT INTO custom_domain_certs
                     (domain, cert_pem, key_pem, issued_at, renew_after)
                 VALUES ($1, $2, $3, $4, $5)
                 ON CONFLICT (domain) DO UPDATE
                   SET cert_pem = EXCLUDED.cert_pem,
                       key_pem = EXCLUDED.key_pem,
                       issued_at = EXCLUDED.issued_at,
                       renew_after = EXCLUDED.renew_after",
                &[&domain, &cert_pem, &key_pem, &issued_at, &renew_after],
            )
            .await?;
        Ok(())
    }

    pub async fn delete_certificate(&self, domain: &str) -> anyhow::Result<()> {
        let conn = self.pool().acquire().await?;
        conn.client()
            .execute(
                "DELETE FROM custom_domain_certs WHERE domain = $1",
                &[&domain],
            )
            .await?;
        Ok(())
    }

    /// Every stored certificate. Used to rebuild the Traefik dynamic
    /// directory, which may be an empty volume on a fresh host.
    pub async fn list_certificates(&self) -> anyhow::Result<Vec<StoredCertificate>> {
        let conn = self.pool().acquire().await?;
        let rows = conn
            .client()
            .query(
                "SELECT domain, cert_pem, key_pem, issued_at, renew_after
                 FROM custom_domain_certs ORDER BY domain",
                &[],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| StoredCertificate {
                domain: r.get(0),
                cert_pem: r.get(1),
                key_pem: r.get(2),
                issued_at: r.get(3),
                renew_after: r.get(4),
            })
            .collect())
    }

    /// Domains whose certificate is due for renewal, plus domains that have
    /// no certificate at all (a first issuance that failed, or one added
    /// while the ACME server was unreachable).
    pub async fn domains_needing_certificates(
        &self,
        now: i64,
    ) -> anyhow::Result<Vec<super::CustomDomainRecord>> {
        let conn = self.pool().acquire().await?;
        let rows = conn
            .client()
            .query(
                "SELECT cd.id, cd.deployment_id, cd.domain, cd.cert_state, cd.created_at,
                        cd.last_error, cd.kind
                 FROM custom_domains cd
                 LEFT JOIN custom_domain_certs c ON c.domain = cd.domain
                 WHERE c.domain IS NULL OR c.renew_after <= $1
                 ORDER BY cd.domain",
                &[&now],
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| super::CustomDomainRecord {
                id: r.get(0),
                deployment_id: r.get(1),
                domain: r.get(2),
                cert_state: r.get(3),
                created_at: r.get(4),
                last_error: r.get(5),
                kind: r.get(6),
            })
            .collect())
    }
}
