use super::{database_error, response_error, write_audit};
use crate::setup::{AppState, UserRole, unavailable};
use crate::page::{Page, PageQuery};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    routing::{delete, get, post, put},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{FromRow, PgPool};
use std::{collections::HashSet, io::Cursor, net::IpAddr, str::FromStr, sync::Arc};
use uuid::Uuid;
use x509_parser::prelude::{FromDer, X509Certificate};

pub fn admin_router() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/api/v1/admin/certificates",
            get(list_admin_certificates).post(create_admin_certificate),
        )
        .route(
            "/api/v1/admin/certificates/{id}",
            put(update_admin_certificate).delete(delete_admin_certificate),
        )
}

pub fn user_router() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/api/v1/certificates",
            get(list_user_certificates).post(create_user_certificate),
        )
        .route(
            "/api/v1/certificates/{id}",
            put(update_user_certificate).delete(delete_user_certificate),
        )
}

#[derive(Debug, Serialize, FromRow)]
struct CertificateRow {
    id: Uuid,
    owner_user_id: Option<Uuid>,
    domains: Vec<String>,
    not_before: String,
    not_after: String,
    issuer: String,
    source: String,
}

#[derive(Debug, Serialize, FromRow)]
struct AdminCertificateRow {
    id: Uuid,
    owner_user_id: Option<Uuid>,
    owner_email: Option<String>,
    domains: Vec<String>,
    not_before: String,
    not_after: String,
    issuer: String,
    source: String,
    created_at: String,
}

const ADMIN_CERTIFICATE_SQL: &str = "SELECT c.id, c.owner_user_id, u.email AS owner_email, c.domains, c.not_before::text AS not_before, c.not_after::text AS not_after, c.issuer, c.source, c.created_at::text AS created_at FROM certificates c LEFT JOIN users u ON u.id = c.owner_user_id";

async fn list_admin_certificates(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(page): Query<PageQuery>,
) -> Result<Json<Page<AdminCertificateRow>>, (StatusCode, Json<Value>)> {
    let (pg, _) = state
        .database_for(&headers, UserRole::Admin)
        .await
        .map_err(response_error)?;
    let (page, page_size, offset) = page.resolve();
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*)::bigint FROM certificates")
        .fetch_one(&pg)
        .await
        .map_err(database_error)?;
    let mut certificates = sqlx::query_as::<_, AdminCertificateRow>(&format!(
        "{ADMIN_CERTIFICATE_SQL} ORDER BY c.created_at DESC LIMIT $1 OFFSET $2"
    ))
    .bind(page_size)
    .bind(offset)
    .fetch_all(&pg)
    .await
    .map_err(database_error)?;
    backfill_missing_issuers(&pg, &mut certificates).await;
    Ok(Json(Page::new(certificates, total, page, page_size)))
}

async fn list_user_certificates(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(page): Query<PageQuery>,
) -> Result<Json<Page<CertificateRow>>, (StatusCode, Json<Value>)> {
    let (pg, user) = state
        .database_for(&headers, UserRole::User)
        .await
        .map_err(response_error)?;
    let (page, page_size, offset) = page.resolve();
    let total: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)::bigint FROM certificates WHERE (owner_user_id = $1 OR owner_user_id IS NULL) AND not_after > now()",
    )
    .bind(user.user_id)
    .fetch_one(&pg)
    .await
    .map_err(database_error)?;
    let mut certificates = sqlx::query_as::<_, CertificateRow>(
        "SELECT id, owner_user_id, domains, not_before::text AS not_before, not_after::text AS not_after, issuer, source FROM certificates WHERE (owner_user_id = $1 OR owner_user_id IS NULL) AND not_after > now() ORDER BY not_after DESC LIMIT $2 OFFSET $3",
    )
    .bind(user.user_id)
    .bind(page_size)
    .bind(offset)
    .fetch_all(&pg)
    .await
    .map_err(database_error)?;
    for row in certificates.iter_mut() {
        backfill_issuer_field(&pg, row.id, &mut row.issuer).await;
    }
    Ok(Json(Page::new(certificates, total, page, page_size)))
}

#[derive(Deserialize)]
struct CreateCertificate {
    certificate_pem: String,
    private_key_pem: String,
}

#[derive(Deserialize)]
struct AdminCreateCertificate {
    #[serde(default)]
    owner_user_id: Option<Uuid>,
    certificate_pem: String,
    private_key_pem: String,
}

#[derive(Deserialize)]
struct AdminUpdateCertificate {
    #[serde(default)]
    owner_user_id: Option<Uuid>,
    #[serde(default)]
    certificate_pem: String,
    #[serde(default)]
    private_key_pem: String,
}

#[derive(Deserialize)]
struct UserUpdateCertificate {
    #[serde(default)]
    certificate_pem: String,
    #[serde(default)]
    private_key_pem: String,
}

async fn create_admin_certificate(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<AdminCreateCertificate>,
) -> Result<(StatusCode, Json<AdminCertificateRow>), (StatusCode, Json<Value>)> {
    let (pg, actor) = state
        .database_for(&headers, UserRole::Admin)
        .await
        .map_err(response_error)?;
    let owner = resolve_owner(&pg, request.owner_user_id).await?;
    let (_, Json(created)) = create_certificate(
        pg.clone(),
        actor.user_id,
        owner,
        CreateCertificate {
            certificate_pem: request.certificate_pem,
            private_key_pem: request.private_key_pem,
        },
    )
    .await?;
    let row = fetch_admin_certificate(&pg, created.id).await?;
    crate::ws::push_node_configs_to_online_nodes(&pg).await;
    Ok((StatusCode::CREATED, Json(row)))
}

async fn update_admin_certificate(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(request): Json<AdminUpdateCertificate>,
) -> Result<Json<AdminCertificateRow>, (StatusCode, Json<Value>)> {
    let (pg, actor) = state
        .database_for(&headers, UserRole::Admin)
        .await
        .map_err(response_error)?;
    let owner = resolve_owner(&pg, request.owner_user_id).await?;
    let (certificate_pem, private_key_pem) = certificate_material_for_update(&pg, id, &request).await?;
    let parsed = parse_certificate(&certificate_pem, &private_key_pem)?;
    let mut transaction = pg.begin().await.map_err(database_error)?;
    let updated = sqlx::query(
        "UPDATE certificates SET owner_user_id = $2, domains = $3, cert_pem = $4, private_key_pem = $5, not_before = $6, not_after = $7, issuer = $8 WHERE id = $1",
    )
    .bind(id)
    .bind(owner)
    .bind(&parsed.domains)
    .bind(&certificate_pem)
    .bind(&private_key_pem)
    .bind(parsed.not_before)
    .bind(parsed.not_after)
    .bind(&parsed.issuer)
    .execute(&mut *transaction)
    .await
    .map_err(database_error)?;
    if updated.rows_affected() == 0 {
        return Err(response_error(unavailable(StatusCode::NOT_FOUND, "证书不存在")));
    }
    write_audit(&mut transaction, actor.user_id, "certificate.update", "certificate", id).await?;
    transaction.commit().await.map_err(database_error)?;
    let row = fetch_admin_certificate(&pg, id).await?;
    crate::ws::push_node_configs_to_online_nodes(&pg).await;
    Ok(Json(row))
}

async fn delete_admin_certificate(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, (StatusCode, Json<Value>)> {
    let (pg, actor) = state
        .database_for(&headers, UserRole::Admin)
        .await
        .map_err(response_error)?;
    let mut transaction = pg.begin().await.map_err(database_error)?;
    let deleted = sqlx::query("DELETE FROM certificates WHERE id = $1")
        .bind(id)
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
    if deleted.rows_affected() == 0 {
        return Err(response_error(unavailable(StatusCode::NOT_FOUND, "证书不存在")));
    }
    write_audit(&mut transaction, actor.user_id, "certificate.delete", "certificate", id).await?;
    transaction.commit().await.map_err(database_error)?;
    crate::ws::push_node_configs_to_online_nodes(&pg).await;
    Ok(StatusCode::NO_CONTENT)
}

async fn create_user_certificate(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<CreateCertificate>,
) -> Result<(StatusCode, Json<CertificateRow>), (StatusCode, Json<Value>)> {
    let (pg, actor) = state.database_for(&headers, UserRole::User).await.map_err(response_error)?;
    create_certificate(pg, actor.user_id, Some(actor.user_id), request).await
}

async fn update_user_certificate(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(request): Json<UserUpdateCertificate>,
) -> Result<Json<CertificateRow>, (StatusCode, Json<Value>)> {
    let (pg, actor) = state
        .database_for(&headers, UserRole::User)
        .await
        .map_err(response_error)?;
    let owned: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM certificates WHERE id = $1 AND owner_user_id = $2)",
    )
    .bind(id)
    .bind(actor.user_id)
    .fetch_one(&pg)
    .await
    .map_err(database_error)?;
    if !owned {
        return Err(response_error(unavailable(StatusCode::NOT_FOUND, "证书不存在")));
    }
    let material = AdminUpdateCertificate {
        owner_user_id: Some(actor.user_id),
        certificate_pem: request.certificate_pem,
        private_key_pem: request.private_key_pem,
    };
    let (certificate_pem, private_key_pem) =
        certificate_material_for_update(&pg, id, &material).await?;
    let parsed = parse_certificate(&certificate_pem, &private_key_pem)?;
    let mut transaction = pg.begin().await.map_err(database_error)?;
    let updated = sqlx::query(
        "UPDATE certificates SET domains = $2, cert_pem = $3, private_key_pem = $4, not_before = $5, not_after = $6, issuer = $7 WHERE id = $1 AND owner_user_id = $8",
    )
    .bind(id)
    .bind(&parsed.domains)
    .bind(&certificate_pem)
    .bind(&private_key_pem)
    .bind(parsed.not_before)
    .bind(parsed.not_after)
    .bind(&parsed.issuer)
    .bind(actor.user_id)
    .execute(&mut *transaction)
    .await
    .map_err(database_error)?;
    if updated.rows_affected() == 0 {
        return Err(response_error(unavailable(StatusCode::NOT_FOUND, "证书不存在")));
    }
    write_audit(
        &mut transaction,
        actor.user_id,
        "certificate.update",
        "certificate",
        id,
    )
    .await?;
    transaction.commit().await.map_err(database_error)?;
    let row = sqlx::query_as::<_, CertificateRow>(
        "SELECT id, owner_user_id, domains, not_before::text AS not_before, not_after::text AS not_after, issuer, source FROM certificates WHERE id = $1",
    )
    .bind(id)
    .fetch_one(&pg)
    .await
    .map_err(database_error)?;
    crate::ws::push_node_configs_to_online_nodes(&pg).await;
    Ok(Json(row))
}

async fn delete_user_certificate(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, (StatusCode, Json<Value>)> {
    let (pg, actor) = state
        .database_for(&headers, UserRole::User)
        .await
        .map_err(response_error)?;
    let mut transaction = pg.begin().await.map_err(database_error)?;
    let deleted = sqlx::query(
        "DELETE FROM certificates WHERE id = $1 AND owner_user_id = $2",
    )
    .bind(id)
    .bind(actor.user_id)
    .execute(&mut *transaction)
    .await
    .map_err(database_error)?;
    if deleted.rows_affected() == 0 {
        return Err(response_error(unavailable(StatusCode::NOT_FOUND, "证书不存在")));
    }
    write_audit(&mut transaction, actor.user_id, "certificate.delete", "certificate", id).await?;
    transaction.commit().await.map_err(database_error)?;
    crate::ws::push_node_configs_to_online_nodes(&pg).await;
    Ok(StatusCode::NO_CONTENT)
}

async fn create_certificate(
    pg: sqlx::PgPool,
    actor_id: Uuid,
    owner_user_id: Option<Uuid>,
    request: CreateCertificate,
) -> Result<(StatusCode, Json<CertificateRow>), (StatusCode, Json<Value>)> {
    let parsed = parse_certificate(&request.certificate_pem, &request.private_key_pem)?;
    let id = Uuid::new_v4();
    let mut transaction = pg.begin().await.map_err(database_error)?;
    let certificate = sqlx::query_as::<_, CertificateRow>(
        "INSERT INTO certificates (id, owner_user_id, domains, cert_pem, private_key_pem, not_before, not_after, issuer, source) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,'manual') RETURNING id, owner_user_id, domains, not_before::text AS not_before, not_after::text AS not_after, issuer, source",
    )
    .bind(id)
    .bind(owner_user_id)
    .bind(&parsed.domains)
    .bind(&request.certificate_pem)
    .bind(&request.private_key_pem)
    .bind(parsed.not_before)
    .bind(parsed.not_after)
    .bind(&parsed.issuer)
    .fetch_one(&mut *transaction)
    .await
    .map_err(database_error)?;
    write_audit(&mut transaction, actor_id, "certificate.upload", "certificate", id).await?;
    transaction.commit().await.map_err(database_error)?;
    Ok((StatusCode::CREATED, Json(certificate)))
}

#[derive(FromRow)]
struct StoredMaterial {
    cert_pem: String,
    private_key_pem: String,
}

struct ParsedCertificate {
    domains: Vec<String>,
    not_before: DateTime<Utc>,
    not_after: DateTime<Utc>,
    issuer: String,
}

fn parse_certificate(
    certificate_pem: &str,
    private_key_pem: &str,
) -> Result<ParsedCertificate, (StatusCode, Json<Value>)> {
    if certificate_pem.len() > 48 * 1024 || private_key_pem.len() > 16 * 1024 {
        return Err(response_error(unavailable(
            StatusCode::BAD_REQUEST,
            "证书或私钥长度无效",
        )));
    }
    let certificates = rustls_pemfile::certs(&mut Cursor::new(certificate_pem.as_bytes()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| response_error(unavailable(StatusCode::BAD_REQUEST, "证书 PEM 格式无效")))?;
    if certificates.is_empty() {
        return Err(response_error(unavailable(StatusCode::BAD_REQUEST, "证书链为空")));
    }
    let (_, pem) = x509_parser::pem::parse_x509_pem(certificate_pem.as_bytes())
        .map_err(|_| response_error(unavailable(StatusCode::BAD_REQUEST, "证书 PEM 格式无效")))?;
    let (rest, certificate) = X509Certificate::from_der(&pem.contents)
        .map_err(|_| response_error(unavailable(StatusCode::BAD_REQUEST, "X.509 证书内容无效")))?;
    if !rest.is_empty() {
        return Err(response_error(unavailable(StatusCode::BAD_REQUEST, "X.509 证书尾部存在无效数据")));
    }
    let not_before = DateTime::<Utc>::from_timestamp(certificate.validity().not_before.timestamp(), 0)
        .ok_or_else(|| response_error(unavailable(StatusCode::BAD_REQUEST, "证书生效时间无效")))?;
    let not_after = DateTime::<Utc>::from_timestamp(certificate.validity().not_after.timestamp(), 0)
        .ok_or_else(|| response_error(unavailable(StatusCode::BAD_REQUEST, "证书过期时间无效")))?;
    if not_before > Utc::now() || not_after <= Utc::now() {
        return Err(response_error(unavailable(StatusCode::BAD_REQUEST, "证书尚未生效或已经过期")));
    }
    let domains = certificate_dns_names(&certificate)?;
    if let Err(message) = rustls_key_matches(&certificates, private_key_pem) {
        return Err(response_error(unavailable(StatusCode::BAD_REQUEST, message)));
    }
    Ok(ParsedCertificate {
        domains,
        not_before,
        not_after,
        issuer: certificate_issuer_name(&certificate),
    })
}

fn certificate_issuer_name(certificate: &X509Certificate<'_>) -> String {
    let issuer = certificate.issuer();
    if let Some(org) = issuer
        .iter_organization()
        .find_map(|attr| attr.as_str().ok().map(str::trim).filter(|s| !s.is_empty()))
    {
        return org.to_owned();
    }
    if let Some(ou) = issuer
        .iter_organizational_unit()
        .find_map(|attr| attr.as_str().ok().map(str::trim).filter(|s| !s.is_empty()))
    {
        return ou.to_owned();
    }
    if let Some(cn) = issuer
        .iter_common_name()
        .find_map(|attr| attr.as_str().ok().map(str::trim).filter(|s| !s.is_empty()))
    {
        return cn.to_owned();
    }
    let raw = issuer.to_string();
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    trimmed.chars().take(200).collect()
}

fn issuer_from_pem(certificate_pem: &str) -> Option<String> {
    let (_, pem) = x509_parser::pem::parse_x509_pem(certificate_pem.as_bytes()).ok()?;
    let (rest, certificate) = X509Certificate::from_der(&pem.contents).ok()?;
    if !rest.is_empty() {
        return None;
    }
    let issuer = certificate_issuer_name(&certificate);
    if issuer.is_empty() {
        None
    } else {
        Some(issuer)
    }
}

async fn backfill_missing_issuers(pg: &PgPool, rows: &mut [AdminCertificateRow]) {
    for row in rows.iter_mut() {
        backfill_issuer_field(pg, row.id, &mut row.issuer).await;
    }
}

async fn backfill_issuer_field(pg: &PgPool, id: Uuid, issuer: &mut String) {
    if !issuer.trim().is_empty() {
        return;
    }
    let pem: Option<String> = sqlx::query_scalar("SELECT cert_pem FROM certificates WHERE id = $1")
        .bind(id)
        .fetch_optional(pg)
        .await
        .ok()
        .flatten();
    let Some(pem) = pem else { return };
    let Some(parsed) = issuer_from_pem(&pem) else { return };
    let _ = sqlx::query("UPDATE certificates SET issuer = $2 WHERE id = $1 AND issuer = ''")
        .bind(id)
        .bind(&parsed)
        .execute(pg)
        .await;
    *issuer = parsed;
}

fn certificate_dns_names(
    certificate: &X509Certificate<'_>,
) -> Result<Vec<String>, (StatusCode, Json<Value>)> {
    let mut domains = Vec::new();
    let mut seen = HashSet::new();
    if let Some(san) = certificate.subject_alternative_name().ok().flatten() {
        for name in &san.value.general_names {
            if let x509_parser::extensions::GeneralName::DNSName(dns) = name {
                push_certificate_domain(&mut domains, &mut seen, dns)?;
            }
        }
    }
    if domains.is_empty() {
        if let Some(name) = certificate.subject().iter_common_name().next() {
            if let Ok(cn) = name.as_str() {
                push_certificate_domain(&mut domains, &mut seen, cn)?;
            }
        }
    }
    if domains.is_empty() {
        return Err(response_error(unavailable(
            StatusCode::BAD_REQUEST,
            "证书没有可用的 DNS 域名",
        )));
    }
    if domains.len() > 100 {
        return Err(response_error(unavailable(
            StatusCode::BAD_REQUEST,
            "证书域名数量超过 100 个",
        )));
    }
    Ok(domains)
}

fn push_certificate_domain(
    domains: &mut Vec<String>,
    seen: &mut HashSet<String>,
    value: &str,
) -> Result<(), (StatusCode, Json<Value>)> {
    let domain = normalize_domain(value)?;
    if seen.insert(domain.clone()) {
        domains.push(domain);
    }
    Ok(())
}

fn rustls_key_matches(
    certificates: &[rustls::pki_types::CertificateDer<'static>],
    private_key_pem: &str,
) -> Result<(), &'static str> {
    let private_key = rustls_pemfile::private_key(&mut Cursor::new(private_key_pem.as_bytes()))
        .map_err(|_| "私钥 PEM 格式无效")?
        .ok_or("私钥 PEM 内容为空")?;
    rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_protocol_versions(&[&rustls::version::TLS13, &rustls::version::TLS12])
    .map_err(|_| "TLS 配置不受支持")?
    .with_no_client_auth()
    .with_single_cert(certificates.to_vec(), private_key)
    .map(|_| ())
    .map_err(|_| "证书与私钥不匹配，或不是受支持的 RSA、ECDSA 证书")
}

async fn certificate_material_for_update(
    pg: &PgPool,
    id: Uuid,
    request: &AdminUpdateCertificate,
) -> Result<(String, String), (StatusCode, Json<Value>)> {
    let certificate_pem = request.certificate_pem.trim();
    let private_key_pem = request.private_key_pem.trim();
    if certificate_pem.is_empty() && private_key_pem.is_empty() {
        let existing = sqlx::query_as::<_, StoredMaterial>(
            "SELECT cert_pem, private_key_pem FROM certificates WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(pg)
        .await
        .map_err(database_error)?;
        let Some(existing) = existing else {
            return Err(response_error(unavailable(StatusCode::NOT_FOUND, "证书不存在")));
        };
        return Ok((existing.cert_pem, existing.private_key_pem));
    }
    if certificate_pem.is_empty() || private_key_pem.is_empty() {
        return Err(response_error(unavailable(
            StatusCode::BAD_REQUEST,
            "更换证书时必须同时提交证书和私钥",
        )));
    }
    Ok((request.certificate_pem.clone(), request.private_key_pem.clone()))
}

async fn resolve_owner(
    pg: &PgPool,
    owner_user_id: Option<Uuid>,
) -> Result<Option<Uuid>, (StatusCode, Json<Value>)> {
    let Some(owner_user_id) = owner_user_id else {
        return Ok(None);
    };
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE id = $1)")
        .bind(owner_user_id)
        .fetch_one(pg)
        .await
        .map_err(database_error)?;
    if !exists {
        return Err(response_error(unavailable(StatusCode::BAD_REQUEST, "所属用户不存在")));
    }
    Ok(Some(owner_user_id))
}

async fn fetch_admin_certificate(
    pg: &PgPool,
    id: Uuid,
) -> Result<AdminCertificateRow, (StatusCode, Json<Value>)> {
    sqlx::query_as::<_, AdminCertificateRow>(&format!("{ADMIN_CERTIFICATE_SQL} WHERE c.id = $1"))
        .bind(id)
        .fetch_optional(pg)
        .await
        .map_err(database_error)?
        .ok_or_else(|| response_error(unavailable(StatusCode::NOT_FOUND, "证书不存在")))
}

fn normalize_domain(value: &str) -> Result<String, (StatusCode, Json<Value>)> {
    let value = value.trim().trim_end_matches('.').to_ascii_lowercase();
    let (wildcard, suffix) = value
        .strip_prefix("*.")
        .map_or((false, value.as_str()), |suffix| (true, suffix));
    if suffix.is_empty() || suffix.len() > 253 || IpAddr::from_str(suffix).is_ok() {
        return Err(response_error(unavailable(StatusCode::BAD_REQUEST, "证书域名格式无效")));
    }
    let subject = if wildcard { format!("probe.{suffix}") } else { suffix.to_owned() };
    let server_name = rustls::pki_types::ServerName::try_from(subject)
        .map_err(|_| response_error(unavailable(StatusCode::BAD_REQUEST, "证书域名格式无效")))?;
    if !matches!(server_name, rustls::pki_types::ServerName::DnsName(_)) {
        return Err(response_error(unavailable(StatusCode::BAD_REQUEST, "证书域名必须是 DNS 名称")));
    }
    Ok(if wildcard { format!("*.{suffix}") } else { suffix.to_owned() })
}
