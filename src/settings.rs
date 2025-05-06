use serde::Deserialize;

#[derive(Deserialize, Debug, Clone)]
pub struct Settings {
    pub client_id: String,
    pub client_secret: String,
    pub zugangsnummer: String,
    pub pin: String,
    pub oauth_url: String,
    pub url: String,
}


