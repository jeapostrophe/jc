use anyhow::{Context, Result, bail};
use serde::Deserialize;

const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const KEYCHAIN_SERVICE: &str = "Claude Code-credentials";

/// Load the OAuth access token from the macOS Keychain.
pub fn load_oauth_token() -> Result<String> {
  let output = std::process::Command::new("security")
    .args(["find-generic-password", "-s", KEYCHAIN_SERVICE, "-w"])
    .output()
    .context("failed to run `security` command")?;

  if !output.status.success() {
    let stderr = String::from_utf8_lossy(&output.stderr);
    bail!("keychain lookup failed: {stderr}");
  }

  let json_str = String::from_utf8(output.stdout)
    .context("keychain output is not valid UTF-8")?
    .trim()
    .to_string();

  let creds: serde_json::Value =
    serde_json::from_str(&json_str).context("failed to parse keychain JSON")?;

  creds
    .pointer("/claudeAiOauth/accessToken")
    .and_then(|v| v.as_str())
    .map(String::from)
    .context("missing claudeAiOauth.accessToken in keychain credentials")
}

/// Fetch usage data from the Anthropic OAuth usage endpoint.
pub fn fetch_usage(token: &str) -> Result<ApiUsageResponse> {
  let mut response = ureq::get(USAGE_URL)
    .header("Authorization", &format!("Bearer {token}"))
    .header("anthropic-beta", "oauth-2025-04-20")
    .call()
    .context("usage API request failed")?;

  let body: ApiUsageResponse =
    response.body_mut().read_json().context("failed to parse usage API response")?;
  Ok(body)
}

/// Response from `GET /api/oauth/usage`.
#[derive(Debug, Clone, Deserialize)]
pub struct ApiUsageResponse {
  pub five_hour: ApiWindow,
  pub seven_day: ApiWindow,
  pub extra_usage: Option<ApiExtraUsage>,
}

/// A usage window (5-hour or 7-day).
#[derive(Debug, Clone, Deserialize)]
pub struct ApiWindow {
  pub utilization: f64,
  pub resets_at: String,
}

/// Extra usage / overage billing info.
#[derive(Debug, Clone, Deserialize)]
pub struct ApiExtraUsage {
  pub is_enabled: bool,
  pub monthly_limit: Option<f64>,
  pub used_credits: Option<f64>,
  pub utilization: Option<f64>,
}
