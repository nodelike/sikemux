// Encryption for PostgreSQL connections, over the same rustls the app uses
// elsewhere. PostgreSQL's own client checks certificates only when told to,
// so the modes here match its `sslmode` names and meanings.

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{verify_tls12_signature, verify_tls13_signature, CryptoProvider};
use rustls::pki_types::{CertificateDer, InvalidDnsNameError, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, SignatureScheme};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio_postgres::tls::{ChannelBinding, MakeTlsConnect, TlsConnect, TlsStream};

use crate::error::{DatabaseError, DatabaseResult};

fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// Takes any certificate, for `prefer` and `require`: the traffic is still
/// encrypted and the handshake still signed, but by whoever answered.
#[derive(Debug)]
struct AnyCertificate(Arc<CryptoProvider>);

impl ServerCertVerifier for AnyCertificate {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

fn tls_error(error: rustls::Error) -> DatabaseError {
    DatabaseError::Connect(format!("tls: {error}"))
}

/// A connector that checks the server's certificate against the system's trusted roots, or not at all.
pub fn connector(verify: bool) -> DatabaseResult<MakeRustls> {
    let provider = provider();
    let builder = ClientConfig::builder_with_provider(Arc::clone(&provider))
        .with_safe_default_protocol_versions()
        .map_err(tls_error)?;
    let config = if verify {
        let verifier = rustls_platform_verifier::Verifier::new(provider).map_err(tls_error)?;
        builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(verifier))
            .with_no_client_auth()
    } else {
        builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(AnyCertificate(provider)))
            .with_no_client_auth()
    };
    Ok(MakeRustls(Arc::new(config)))
}

#[derive(Clone)]
pub struct MakeRustls(Arc<ClientConfig>);

impl<S> MakeTlsConnect<S> for MakeRustls
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    type Stream = RustlsStream<S>;
    type TlsConnect = RustlsConnect;
    type Error = InvalidDnsNameError;

    fn make_tls_connect(&mut self, domain: &str) -> Result<RustlsConnect, InvalidDnsNameError> {
        Ok(RustlsConnect {
            config: Arc::clone(&self.0),
            name: ServerName::try_from(domain.to_string())?,
        })
    }
}

pub struct RustlsConnect {
    config: Arc<ClientConfig>,
    name: ServerName<'static>,
}

impl<S> TlsConnect<S> for RustlsConnect
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    type Stream = RustlsStream<S>;
    type Error = io::Error;
    type Future = Pin<Box<dyn Future<Output = io::Result<RustlsStream<S>>> + Send>>;

    fn connect(self, stream: S) -> Self::Future {
        Box::pin(async move {
            let connector = tokio_rustls::TlsConnector::from(self.config);
            Ok(RustlsStream(connector.connect(self.name, stream).await?))
        })
    }
}

pub struct RustlsStream<S>(tokio_rustls::client::TlsStream<S>);

impl<S: AsyncRead + AsyncWrite + Unpin> AsyncRead for RustlsStream<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_read(context, buffer)
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> AsyncWrite for RustlsStream<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.0).poll_write(context, buffer)
    }

    fn poll_flush(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_flush(context)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_shutdown(context)
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> TlsStream for RustlsStream<S> {
    fn channel_binding(&self) -> ChannelBinding {
        ChannelBinding::none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_connectors_build_and_take_host_names_and_addresses() {
        for verify in [false, true] {
            let mut make = connector(verify).unwrap();
            assert!(MakeTlsConnect::<tokio::net::TcpStream>::make_tls_connect(
                &mut make,
                "db.example.com"
            )
            .is_ok());
            assert!(MakeTlsConnect::<tokio::net::TcpStream>::make_tls_connect(
                &mut make, "10.0.0.5"
            )
            .is_ok());
            assert!(MakeTlsConnect::<tokio::net::TcpStream>::make_tls_connect(
                &mut make,
                "not a host"
            )
            .is_err());
        }
    }
}
