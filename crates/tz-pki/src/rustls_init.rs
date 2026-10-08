use std::sync::{Arc, Once};

static INIT: Once = Once::new();

/// Process-wide rustls backend: AWS-LC (FIPS-capable).
pub fn aws_crypto_provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::aws_lc_rs::default_provider())
}

/// Required for crates that call `ClientConfig::builder()` / `builder_with_protocol_versions` without an explicit provider.
pub fn init_rustls_crypto_provider() {
    INIT.call_once(|| {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    });
}
