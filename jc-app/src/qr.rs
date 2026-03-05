use serde::Serialize;

/// Payload encoded into the QR code for mobile device pairing.
#[derive(Debug, Clone, Serialize)]
pub struct QrPayload {
  pub host: String,
  pub port: u16,
  pub token: String,
  pub fingerprint: String,
}

/// Generate a QR code module grid from a `QrPayload`.
///
/// Returns a 2D boolean grid where `true` = dark module, `false` = light module.
pub fn generate_qr(payload: &QrPayload) -> Vec<Vec<bool>> {
  let json = serde_json::to_string(payload).expect("QrPayload serialization cannot fail");
  let qr = fast_qr::QRBuilder::new(json).build().expect("QR generation failed");

  (0..qr.size).map(|row| qr[row].iter().map(|m| m.value()).collect()).collect()
}
