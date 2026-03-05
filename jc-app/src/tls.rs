use anyhow::Result;
use rcgen::{CertificateParams, KeyPair, SanType};
use sha2::{Digest, Sha256};

/// Generate a self-signed TLS certificate for the mobile server.
///
/// Returns the rustls ServerConfig and the SHA-256 fingerprint of the certificate
/// (hex-encoded, colon-separated) for QR-based cert pinning.
pub fn generate_self_signed() -> Result<(rustls::ServerConfig, String)> {
  let mut params = CertificateParams::default();
  params.subject_alt_names = san_entries();

  let key_pair = KeyPair::generate()?;
  let cert = params.self_signed(&key_pair)?;

  let cert_der = cert.der().clone();
  let key_der = rustls::pki_types::PrivatePkcs8KeyDer::from(key_pair.serialize_der());

  // Compute SHA-256 fingerprint.
  let fingerprint = {
    let digest = Sha256::digest(cert_der.as_ref());
    let hex_bytes: Vec<String> = digest.iter().map(|b| format!("{b:02X}")).collect();
    hex_bytes.join(":")
  };

  let server_config = rustls::ServerConfig::builder()
    .with_no_client_auth()
    .with_single_cert(vec![cert_der], key_der.into())?;

  Ok((server_config, fingerprint))
}

/// Build SAN entries: localhost + any non-loopback IPv4 LAN addresses.
fn san_entries() -> Vec<SanType> {
  let mut sans = vec![
    SanType::DnsName("localhost".try_into().expect("localhost is valid")),
    SanType::IpAddress(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)),
  ];

  // Add LAN IP addresses for direct device connections.
  if let Some(addr) = local_lan_ip() {
    sans.push(SanType::IpAddress(std::net::IpAddr::V4(addr)));
  }

  sans
}

/// Discover the local LAN IPv4 address via the default route.
///
/// Uses the UDP socket trick: bind to 0.0.0.0, "connect" to a public IP (no
/// packets sent), then read back the local address the OS selected.
pub fn local_lan_ip() -> Option<std::net::Ipv4Addr> {
  let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
  socket.connect("8.8.8.8:80").ok()?;
  let addr = socket.local_addr().ok()?;
  match addr.ip() {
    std::net::IpAddr::V4(v4) if !v4.is_loopback() => Some(v4),
    _ => None,
  }
}
