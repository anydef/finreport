use serde::{Deserialize, Serialize};

#[derive(Deserialize, Debug)]
pub struct OAuthResponse {
    pub access_token: String,
    token_type: String,
    refresh_token: String,
    #[serde(rename = "kdnr")]
    client_id: String,
    bpid: u64,
    #[serde(rename = "kontaktId")]
    contact_id: u64,
    expires_in: u32,
    scope: String,
}


#[derive(Deserialize, Debug, Serialize)]
pub struct HttpRequestInfoHeader {
    #[serde(rename = "clientRequestId")]
    client_request_id: ClientRequestId,
}
impl HttpRequestInfoHeader {
    pub fn from(session_id: String, request_id: String) -> Self {
        HttpRequestInfoHeader {
            client_request_id: ClientRequestId {
                session_id,
                request_id,
            },
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("Could not serialize info-header")
    }
}

#[derive(Deserialize, Debug, Serialize)]
struct ClientRequestId {
    #[serde(rename = "sessionId")]
    session_id: String,
    #[serde(rename = "requestId")]
    request_id: String,
}

#[derive(Deserialize, Serialize, Debug, Default, Clone)]
pub struct SessionStatus {

    pub(crate) identifier: String,

    #[serde(rename = "sessionTanActive")]
    pub(crate) session_tan_active: bool,
    #[serde(rename = "activated2FA")]
    pub(crate) activated_2fa: bool,
}

impl SessionStatus {
    pub(crate) fn is_valid(&self) -> bool {
        self.session_tan_active && self.activated_2fa
    }
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct AuthenticationInfo {
    #[serde(rename = "id")]
    pub challenge_id: String,
    typ: String,
    #[serde(rename = "availableTypes")]
    available_types: Vec<String>,
    link: AuthenticationInfoLink,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
struct AuthenticationInfoLink {
    rel: String,
    method: String,
    #[serde(rename = "type")]
    content_type: String,
}



