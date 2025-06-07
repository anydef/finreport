use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use uuid::Uuid;
use crate::comdirect::utils;

#[derive(Debug)]
pub enum SessionClientError {
    Unauthorized,
    Unknown,
}

impl Display for SessionClientError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "Client error: {:?}", self)
    }
}

impl std::error::Error for SessionClientError {}

type SessionClientResult<T> = Result<T, SessionClientError>;

#[derive(Serialize)]
pub struct XOnceAuthenticationInfo {
    #[serde(rename = "id")]
    pub challenge_id: String,
}

pub struct SessionClient {
    client: Client,

    url: String,
    oauth_url: String,
    client_id: String,
    client_secret: String,
    username: String,
    password: String,
    session_id: String,
}

impl SessionClient {
    pub fn new(
        url: String,
        oauth_url: String,
        client_id: String,
        client_secret: String,
        username: String,
        password: String,
        client: Client,
    ) -> Self {
        Self {
            client,
            url,
            oauth_url,
            client_id,
            client_secret,
            username,
            password,
            session_id: Uuid::new_v4().to_string(),
        }
    }
}

impl SessionClient {
    pub async fn get_session_status(
        &mut self,
        session: &Session,
    ) -> SessionClientResult<SessionStatus> {
        let result = self
            .client
            .get(format!("{}/session/clients/user/v1/sessions", self.url))
            .header(ACCEPT, "application/json")
            .header(AUTHORIZATION, format!("Bearer {}", session.access_token))
            .header("x-http-request-info", self.info_header())
            .send()
            .await;

        if let Ok(r) = result {
            match r.status() {
                StatusCode::OK => {
                    let sessions: Vec<SessionStatus> = r.json().await.unwrap_or_else(|_| vec![]);
                    match sessions.first() {
                        Some(session) => {
                            println!("Session status: {:?}", session);
                            Ok(session.clone())
                        }
                        None => {
                            println!("Error: No Session status available");
                            Err(SessionClientError::Unknown)
                        }
                    }
                }
                StatusCode::UNAUTHORIZED => Err(SessionClientError::Unauthorized),
                _ => {
                    println!("Error: Could not get session status: {}", r.status());
                    Err(SessionClientError::Unknown)
                }
            }
        } else {
            Err(SessionClientError::Unknown)
        }
    }
}

impl SessionClient {
    pub async fn validate_session(
        &mut self,
        session: &Session,
    ) -> SessionClientResult<XOnceAuthenticationInfo> {
        let patched_session = SessionStatus {
            identifier: session.session_uuid.clone(),
            session_tan_active: true,
            activated_2fa: true,
        };

        let validation_result = self
            .client
            .post(format!(
                "{}/session/clients/user/v1/sessions/{}/validate",
                self.url,
                session.session_uuid.clone()
            ))
            .json(&patched_session)
            .header(ACCEPT, "application/json")
            .header("x-http-request-info", self.info_header())
            .header(AUTHORIZATION, format!("Bearer {}", session.access_token))
            .send()
            .await;
        match validation_result {
            Ok(r) => match r.status() {
                StatusCode::CREATED => match r.headers().get("x-once-authentication-info") {
                    Some(header) => {
                        let header_str = header.to_str().unwrap_or_default();
                        let authentication_info: AuthenticationInfo =
                            serde_json::from_str(header_str).unwrap();
                        println!("Authentication Info: {:?}", authentication_info.clone());
                        let challenge_id = authentication_info.challenge_id;
                        Ok(XOnceAuthenticationInfo { challenge_id })
                    }
                    None => {
                        println!("Error: No authentication info available");
                        Err(SessionClientError::Unknown)
                    }
                },
                _ => {
                    println!("Error: Unexpected status code {}", r.status());
                    Err(SessionClientError::Unknown)
                }
            },
            Err(_) => Err(SessionClientError::Unknown),
        }
    }
}

impl SessionClient {
    pub async fn acquire_password_token(&mut self) -> SessionClientResult<OAuthResponse> {
        let params = [
            ("client_id", self.client_id.clone()),
            ("client_secret", self.client_secret.clone()),
            ("username", self.username.clone()),
            ("password", self.password.clone()),
            ("grant_type", "password".to_string()),
        ];

        let result = self
            .client
            .post(format!("{}/oauth/token", self.oauth_url))
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .header(ACCEPT, "application/json")
            .form(&params)
            .send()
            .await;

        match result {
            Ok(response) => match response.status().is_success() {
                true => {
                    let oauth_response: OAuthResponse = response.json().await.unwrap();
                    Ok(oauth_response)
                }
                false => Err(SessionClientError::Unknown),
            },
            Err(_) => Err(SessionClientError::Unknown),
        }
    }
}

impl SessionClient {
    pub async fn patch_session(
        &mut self,
        session: &Session,
        x_once_oauth_info: &XOnceAuthenticationInfo,
    ) -> SessionClientResult<SessionStatus> {
        let patched = SessionStatus {
            identifier: session.session_uuid.clone(),
            session_tan_active: true,
            activated_2fa: true,
        };
        let patched_session = self
            .client
            .patch(format!(
                "{}/session/clients/user/v1/sessions/{}",
                self.url, session.session_uuid
            ))
            .json(&patched)
            .header(
                AUTHORIZATION,
                format!("Bearer {}", session.access_token.clone()),
            )
            // .header(CONTENT_TYPE, "application/json")
            .header(
                "x-once-authentication-info",
                serde_json::to_string(x_once_oauth_info).expect("Failed to serialize challenge"),
            )
            .header(ACCEPT, "application/json")
            .header("x-http-request-info", self.info_header())
            .header("x-once-authentication", "000000")
            .send()
            .await;

        match patched_session {
            Ok(r) => match r.status() {
                StatusCode::OK => {
                    println!("Session activated successfully: {:?}", r.status());

                    let session_status: reqwest::Result<SessionStatus> = r.json().await;
                    if let Ok(session_status) = session_status {
                        Ok(session_status)
                    } else {
                        println!("Error: No session status available");
                        Err(SessionClientError::Unknown)
                    }
                }
                _ => {
                    println!("Error: Unexpected status code {}", r.status());
                    Err(SessionClientError::Unknown)
                }
            },
            Err(_) => {
                println!("Error: Could not send request to validate session");
                Err(SessionClientError::Unknown)
            }
        }
    }
}

impl SessionClient {
    pub async fn activate_secondary_flow(
        &mut self,
        session: &Session,
    ) -> SessionClientResult<Session> {
        let params = [
            ("client_id", self.client_id.clone()),
            ("client_secret", self.client_secret.clone()),
            ("token", session.access_token.clone()),
            ("grant_type", "cd_secondary".to_string()),
        ];
        let oauth_cd_secondary_flow = self
            .client
            .post(format!("{}/oauth/token", self.oauth_url))
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .header(ACCEPT, "application/json")
            .form(&params)
            .send()
            .await;
        match oauth_cd_secondary_flow {
            Ok(r) => match r.status() {
                StatusCode::OK => {
                    let oauth_response: reqwest::Result<OAuthResponse> = r.json().await;
                    if let Ok(oauth_response) = oauth_response {
                        println!("Access Token: {}", oauth_response.access_token);
                        Ok(Session::from_oauth(oauth_response))
                    } else {
                        println!("Error: Could not parse JSON response");
                        Err(SessionClientError::Unknown)
                    }
                }
                _ => {
                    println!("Error: Unexpected status code {}", r.status());
                    Err(SessionClientError::Unknown)
                }
            },
            Err(_) => {
                println!("Error: Could not send request to validate session");
                Err(SessionClientError::Unknown)
            }
        }
    }
}

impl SessionClient {
    pub async fn refresh_token_flow(
        &mut self,
        session: &Session,
    ) -> SessionClientResult<OAuthResponse> {
        let params = [
            ("client_id", self.client_id.clone()),
            ("client_secret", self.client_secret.clone()),
            ("refresh_token", session.refresh_token.clone()),
            ("grant_type", "refresh_token".to_string()),
        ];

        let refresh_token_result = self
            .client
            .post(format!("{}/oauth/token", self.oauth_url))
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .header(ACCEPT, "application/json")
            .form(&params)
            .send()
            .await;
        match refresh_token_result {
            Ok(r) => match r.status() {
                StatusCode::OK => {
                    let oauth_response: reqwest::Result<OAuthResponse> = r.json().await;
                    if let Ok(oauth_response) = oauth_response {
                        Ok(oauth_response)
                    } else {
                        println!("Error: Could not parse JSON response");
                        Err(SessionClientError::Unknown)
                    }
                }
                _ => {
                    println!("Error: Unexpected status code {}", r.status());
                    Err(SessionClientError::Unknown)
                }
            },
            Err(_) => {
                println!("Error: Could not send request to validate session");
                Err(SessionClientError::Unknown)
            }
        }
    }
}

impl SessionClient {
    fn info_header(&self) -> String {
        let info_header = HttpRequestInfoHeader::from(self.session_id.clone(), utils::request_id());
        serde_json::to_string(&info_header).expect("Could not serialize info-header")
    }
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Default, Clone)]
pub struct Session {
    pub access_token: String,
    pub session_uuid: String,
    pub refresh_token: String,
}

impl Session {
    pub fn from_oauth(oauth_response: OAuthResponse) -> Session {
        Session {
            access_token: oauth_response.access_token,
            session_uuid: Uuid::new_v4().to_string(),
            refresh_token: oauth_response.refresh_token,
        }
    }

    pub fn refreshed_session(&self, oauth_response: OAuthResponse) -> Self {
        let mut new_session = self.clone();
        new_session.access_token = oauth_response.access_token;
        new_session.refresh_token = oauth_response.refresh_token;

        new_session
    }
}

#[derive(Deserialize, Debug)]
pub struct OAuthResponse {
    pub access_token: String,
    token_type: String,
    pub refresh_token: String,
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

#[derive(Deserialize, Serialize, Debug, Default, Clone)]
pub struct SessionStatus {
    pub identifier: String,

    #[serde(rename = "sessionTanActive")]
    pub session_tan_active: bool,
    #[serde(rename = "activated2FA")]
    pub activated_2fa: bool,
}

#[derive(Deserialize, Debug, Serialize)]
pub struct ClientRequestId {
    #[serde(rename = "sessionId")]
    session_id: String,
    #[serde(rename = "requestId")]
    request_id: String,
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