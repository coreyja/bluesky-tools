use cja::{color_eyre, Result};
use serde::{Deserialize, Serialize};

#[derive(Clone)]
pub struct TwilioConfig {
    pub account_sid: String,
    pub auth_token: String,
    pub phone_number: String,
}

impl TwilioConfig {
    pub fn from_env() -> Result<Self> {
        Ok(Self {
            account_sid: std::env::var("TWILIO_ACCOUNT_SID")?,
            auth_token: std::env::var("TWILIO_AUTH_TOKEN")?,
            phone_number: std::env::var("TWILIO_PHONE_NUMBER")?,
        })
    }
}

pub async fn send_sms(config: &TwilioConfig, to: &str, body: &str) -> Result<()> {
    let client = reqwest::Client::new();
    let url = format!(
        "https://api.twilio.com/2010-04-01/Accounts/{}/Messages.json",
        config.account_sid
    );

    let resp = client
        .post(url)
        .basic_auth(config.account_sid.clone(), Some(config.auth_token.clone()))
        .form(&[("To", to), ("From", &config.phone_number), ("Body", body)])
        .send()
        .await?;

    if !resp.status().is_success() {
        return Err(color_eyre::eyre::eyre!("failed to send sms"));
    }

    Ok(())
}

#[derive(Serialize, Deserialize)]
pub struct VerifiedPhoneNumber {
    pub calling_country_code: String,
    pub country_code: String,
    pub national_format: String,
    pub phone_number: String,
    pub url: String,
    pub valid: bool,
}

pub async fn find_verified_phone_numbers(
    config: &TwilioConfig,
    input_number: &str,
) -> Result<VerifiedPhoneNumber> {
    let client = reqwest::Client::new();
    let url = format!(
        "https://lookups.twilio.com/v2/PhoneNumbers/{}",
        input_number
    );

    let resp = client
        .get(url)
        .basic_auth(config.account_sid.clone(), Some(config.auth_token.clone()))
        .send()
        .await?
        .json::<VerifiedPhoneNumber>()
        .await?;

    Ok(resp)
}
