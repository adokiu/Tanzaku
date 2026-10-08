pub use tz_pki::IssuedCertificate;

pub async fn load_ca_certificate_pem(pool: &sqlx::PgPool) -> anyhow::Result<String> {
    sqlx::query_scalar("SELECT certificate_pem FROM pki_ca WHERE id = 1")
        .fetch_one(pool)
        .await
        .map_err(|_| anyhow::anyhow!("internal CA is missing"))
}

pub async fn sign_csr(pool: &sqlx::PgPool, csr_pem: &str) -> anyhow::Result<IssuedCertificate> {
    let (ca_cert_pem, ca_key_pem) = sqlx::query_as::<_, (String, String)>(
        "SELECT certificate_pem, private_key_pem FROM pki_ca WHERE id = 1",
    )
    .fetch_one(pool)
    .await
    .map_err(|_| anyhow::anyhow!("internal CA is missing"))?;
    tz_pki::sign_csr_pem_with_ca(&ca_cert_pem, &ca_key_pem, csr_pem)
}
