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
  if let Ok(addrs) = local_ipv4_addresses() {
    for addr in addrs {
      sans.push(SanType::IpAddress(std::net::IpAddr::V4(addr)));
    }
  }

  sans
}

/// Discover non-loopback IPv4 addresses on this machine.
fn local_ipv4_addresses() -> Result<Vec<std::net::Ipv4Addr>> {
  let mut addrs = Vec::new();
  // Use a UDP socket trick to discover the default route address.
  if let Ok(socket) = std::net::UdpSocket::bind("0.0.0.0:0")
    && socket.connect("8.8.8.8:80").is_ok()
    && let Ok(local_addr) = socket.local_addr()
    && let std::net::IpAddr::V4(v4) = local_addr.ip()
    && !v4.is_loopback()
  {
    addrs.push(v4);
  }
  Ok(addrs)
}
