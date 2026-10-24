// SPDX-License-Identifier: EPL-2.0

//! The secrets (spec §14.5), read once at cold start: the SSM SecureStrings
//! `<prefix>/signing-key` (`kid:hex` lines, for batch tokens and OAuth state) and
//! `<prefix>/github-client-secret`.

use rumble_core::token::Keys;

pub struct Secrets {
    pub keys: Keys,
    pub github_client_secret: String,
}

async fn parameter(client: &aws_sdk_ssm::Client, name: &str) -> Result<String, String> {
    let output = client
        .get_parameter()
        .name(name)
        .with_decryption(true)
        .send()
        .await
        .map_err(|e| {
            format!(
                "SSM {name}: {}",
                aws_sdk_ssm::error::DisplayErrorContext(&e)
            )
        })?;
    output
        .parameter
        .and_then(|p| p.value)
        .ok_or_else(|| format!("SSM {name} has no value"))
}

/// Reads both parameters under `prefix` (`/robocode-s/<stage>`).
pub async fn load(client: &aws_sdk_ssm::Client, prefix: &str) -> Result<Secrets, String> {
    let prefix = prefix.trim_end_matches('/');
    let keys = parameter(client, &format!("{prefix}/signing-key")).await?;
    let secret = parameter(client, &format!("{prefix}/github-client-secret")).await?;
    Ok(Secrets {
        keys: Keys::parse(&keys)?,
        github_client_secret: secret.trim().to_string(),
    })
}
