//! TLS materials for the daemon's two servers: load and validate the
//! optional `KRONIKA_TLS_CERT` / `KRONIKA_TLS_KEY` pair before any listener
//! binds, and warn loudly on network-exposed plaintext.

use anyhow::{Context, Result};
use tumult_ingest::Config;

/// Validated TLS materials for both servers: the rustls config for the
/// HTTPS (axum) listener and the PEM identity for the tonic gRPC server.
pub(super) struct TlsMaterials {
    pub http: axum_server::tls_rustls::RustlsConfig,
    pub grpc_identity: tonic::transport::Identity,
}

/// Load the optional TLS configuration (`KRONIKA_TLS_CERT` /
/// `KRONIKA_TLS_KEY`). Returns `None` when TLS is not configured; when it is,
/// the certificate chain and key are parsed here so a bad pair fails startup
/// with a clear message before any listener binds.
pub(super) async fn load_tls(config: &Config) -> Result<Option<TlsMaterials>> {
    let (Some(cert), Some(key)) = (config.tls_cert.as_deref(), config.tls_key.as_deref()) else {
        return Ok(None);
    };
    // rustls has both the ring and aws-lc-rs providers compiled in via the
    // wider dependency graph; without an explicit process default, building a
    // `ServerConfig` panics on the ambiguity. Ignore "already installed".
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let http = axum_server::tls_rustls::RustlsConfig::from_pem_file(cert, key)
        .await
        .with_context(|| {
            format!(
                "load TLS certificate chain and key from KRONIKA_TLS_CERT ({}) and \
                 KRONIKA_TLS_KEY ({})",
                cert.display(),
                key.display()
            )
        })?;
    let cert_pem = std::fs::read(cert)
        .with_context(|| format!("read KRONIKA_TLS_CERT ({})", cert.display()))?;
    let key_pem =
        std::fs::read(key).with_context(|| format!("read KRONIKA_TLS_KEY ({})", key.display()))?;
    let grpc_identity = tonic::transport::Identity::from_pem(cert_pem, key_pem);
    tracing::info!(
        cert = %cert.display(),
        "TLS enabled for the HTTP and gRPC servers"
    );
    Ok(Some(TlsMaterials {
        http,
        grpc_identity,
    }))
}

/// Loud startup warning when a network-exposed listener serves plaintext:
/// bearer tokens and telemetry cross the wire unencrypted.
pub(super) fn warn_if_plaintext_on_network(config: &Config, tls_enabled: bool) {
    if tls_enabled {
        return;
    }
    if !config.otlp_http_addr.ip().is_loopback() {
        tracing::warn!(
            addr = %config.otlp_http_addr,
            "TLS is OFF: the API, web UI and OTLP/HTTP ingest are served in plaintext on a \
             network interface — bearer tokens and telemetry cross the wire unencrypted; \
             set KRONIKA_TLS_CERT/KRONIKA_TLS_KEY or terminate TLS at a reverse proxy"
        );
    }
    if !config.otlp_grpc_addr.ip().is_loopback() {
        tracing::warn!(
            addr = %config.otlp_grpc_addr,
            "TLS is OFF: OTLP/gRPC ingest is served in plaintext on a network interface; \
             set KRONIKA_TLS_CERT/KRONIKA_TLS_KEY or terminate TLS at a reverse proxy"
        );
    }
}
