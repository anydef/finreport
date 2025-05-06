use crate::comdirect::session::SessionState::NotInitialized;
use crate::comdirect::session_model::{
    AuthenticationInfo, HttpRequestInfoHeader, OAuthResponse, SessionStatus,
};
use crate::settings::Settings;
use crate::utils::wait_user_input;
use reqwest::StatusCode;
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use reqwest::{Client, Error, Response};
use std::cmp::PartialEq;
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

#[derive(PartialOrd, PartialEq)]
enum SessionState {
    NotInitialized,
    LoginSuccess(String),
    SessionInactive,
    SessionValidated,
    SessionActivated,
    SecondaryFlowSessionActivated,
    Failed(String),
}

trait AcquirePasswordToken {
    async fn acquire_password_token(&mut self);
}
trait AuthSessionStatus {
    async fn get_session_status(&mut self) -> SessionStatus;
}
trait ValidateSession {
    async fn validate_session(&mut self);
}
trait ActivateSessionTan {
    async fn activate_session_tan(&self);
}
trait SecondaryFlow {
    async fn extend_session_cd_secondary_flow(&mut self);
}
pub trait ClientSession {
    fn client(&self) -> &Client;
    fn url(&self) -> String;
}
pub trait TokenAware {
    fn access_token(&self) -> Option<String>;
    fn info_header(&self) -> String;
}

pub struct Session {
    state: SessionState,
    client_id: String,
    client_secret: String,
    username: String,
    password: String,
    oauth_url: String,
    url: String,
    client: Client,
    access_token: Option<String>,
    session_id: Option<String>,
    session_status: SessionStatus,
    challenage_id: Option<String>,
}

impl TokenAware for Session {
    fn access_token(&self) -> Option<String> {
        self.access_token.clone()
    }

    fn info_header(&self) -> String {
        let info_header =
            HttpRequestInfoHeader::from(self.session_id.clone().unwrap_or_default(), request_id());
        serde_json::to_string(&info_header).expect("Could not serialize info-header")
    }
}
impl ClientSession for Session {
    fn client(&self) -> &Client {
        &self.client
    }

    fn url(&self) -> String {
        self.url.clone()
    }
}

impl Session {
    pub fn new(settings: Settings, client: Client) -> Self {
        Session {
            state: NotInitialized,
            client_id: settings.client_id,
            client_secret: settings.client_secret,
            username: settings.zugangsnummer,
            password: settings.pin,
            oauth_url: settings.oauth_url,
            url: settings.url,
            client,
            access_token: None,
            session_id: None,
            session_status: SessionStatus::default(),
            challenage_id: None,
        }
    }
}

impl Session {
    pub async fn login(&mut self) {
        self.acquire_password_token().await;
        let session_status = self.get_session_status().await;

        if !session_status.is_valid() {
            self.validate_session().await;
            wait_user_input()
                .await
                .expect("Failed waiting for user input");
            self.activate_session_tan().await;
        }
        self.extend_session_cd_secondary_flow().await;
    }
}

impl AcquirePasswordToken for Session {
    async fn acquire_password_token(&mut self) {
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
                    self.access_token = Some(oauth_response.access_token.clone());
                    println!("Access Token: {}", oauth_response.access_token);
                }
                false => {
                    println!("Error: Could not acquire password token");
                    self.access_token.take();
                }
            },
            Err(_) => {
                self.access_token.take();
                println!("Error: Could not send request");
            }
        };
    }
}

impl AuthSessionStatus for Session {
    async fn get_session_status(&mut self) -> SessionStatus {
        let default_session = SessionStatus::default();

        if let Some(token) = &self.access_token {
            let session_id = Uuid::new_v4().to_string();
            self.session_id = Some(session_id.clone());
            let info_header = HttpRequestInfoHeader::from(session_id.clone(), request_id());

            let info_header_json =
                serde_json::to_string(&info_header).expect("Could not serialize info-header");
            let session_status_response = self
                .client
                .get(format!("{}/session/clients/user/v1/sessions", self.url))
                .header(ACCEPT, "application/json")
                .header(AUTHORIZATION, format!("Bearer {}", token))
                .header("x-http-request-info", info_header_json.clone())
                .send()
                .await;
            let session_status = if let Ok(r) = session_status_response {
                match r.status() {
                    StatusCode::OK => {
                        println!("Session status: {:?}", r.status());
                        let session_status: Vec<SessionStatus> =
                            r.json().await.unwrap_or_else(|e| {
                                println!("Error: Could not parse JSON response: {:}", e);
                                vec![]
                            });
                        let current_session =
                            session_status.first().cloned().unwrap_or_else(|| {
                                println!("Error: No Session status available",);
                                default_session
                            });
                        current_session
                    }
                    _ => {
                        println!("Error: Could not get session status: {}", r.status());
                        default_session
                    }
                }
            } else {
                default_session
            };
            self.session_status = session_status.clone();
            session_status
        } else {
            println!("Error: No access token available",);

            SessionStatus::default()
        }
    }
}
impl ValidateSession for Session {
    async fn validate_session(&mut self) {
        let patched_session = SessionStatus {
            identifier: self.session_status.identifier.clone(),
            session_tan_active: true,
            activated_2fa: true,
        };
        let token = self.access_token.clone().unwrap_or_default();
        let session_id = self.session_id.clone().unwrap_or_default();
        let info_header = HttpRequestInfoHeader::from(session_id, request_id());

        let validated_session_result = self
            .client
            .post(format!(
                "{}/session/clients/user/v1/sessions/{}/validate",
                self.url, self.session_status.identifier
            ))
            .json(&patched_session)
            .header(ACCEPT, "application/json")
            .header("x-http-request-info", info_header.to_json())
            .header(AUTHORIZATION, format!("Bearer {}", token))
            .send()
            .await;

        match validated_session_result {
            Ok(r) => match r.status() {
                StatusCode::CREATED => {
                    println!("Session validated successfully: {:?}", r.status());
                    match r.headers().get("x-once-authentication-info") {
                        Some(header) => {
                            let header_str = header.to_str().unwrap_or_default();
                            let authentication_info: AuthenticationInfo =
                                serde_json::from_str(header_str).unwrap();
                            println!("Authentication Info: {:?}", authentication_info.clone());
                            self.challenage_id = Some(authentication_info.challenge_id);
                        }
                        None => {
                            println!("Error: No authentication info available");
                        }
                    }
                }
                _ => {
                    println!("Error: Unexpected status code {}", r.status())
                }
            },
            Err(_) => {
                println!("Error: Could not send request to validate session");
            }
        };
    }
}
impl ActivateSessionTan for Session {
    async fn activate_session_tan(&self) {
        let patched = SessionStatus {
            identifier: self.session_status.identifier.clone(),
            session_tan_active: true,
            activated_2fa: true,
        };
        let info_header =
            HttpRequestInfoHeader::from(self.session_id.clone().unwrap_or_default(), request_id());
        let mut challenge = HashMap::new();
        challenge.insert("id", self.challenage_id.clone().unwrap_or_default());

        let patched_session = self
            .client
            .patch(format!(
                "{}/session/clients/user/v1/sessions/{}",
                self.url,
                self.session_status.identifier.clone()
            ))
            .json(&patched)
            .header(
                AUTHORIZATION,
                format!("Bearer {}", self.access_token.clone().unwrap_or_default()),
            )
            // .header(CONTENT_TYPE, "application/json")
            .header(
                "x-once-authentication-info",
                serde_json::to_string(&challenge).expect("Failed to serialize challenge"),
            )
            .header(ACCEPT, "application/json")
            .header("x-http-request-info", info_header.to_json())
            .header("x-once-authentication", "000000")
            .send()
            .await;

        match patched_session {
            Ok(r) => match r.status() {
                StatusCode::OK => {
                    println!(
                        "activate_session_tan Session activated successfully: {:?}",
                        r.status()
                    );
                }
                _ => {
                    println!(
                        "activate_session_tan Error: Unexpected status code {}",
                        r.status()
                    );
                }
            },
            Err(_) => {
                println!("activate_session_tan Error: Could not send request to validate session");
            }
        }
    }
}

impl SecondaryFlow for Session {
    async fn extend_session_cd_secondary_flow(&mut self) {
        let params = [
            ("client_id", self.client_id.clone()),
            ("client_secret", self.client_secret.clone()),
            ("token", self.access_token.clone().unwrap_or_default()),
            ("grant_type", "cd_secondary".to_string()),
        ];
        let oauth_cd_secondary_flow = self
            .client
            .post(format!("{}/oauth/token", self.oauth_url))
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .header("Accept", "application/json")
            .form(&params)
            .send()
            .await;
        match oauth_cd_secondary_flow {
            Ok(r) => match r.status() {
                StatusCode::OK => {
                    let oauth_response: OAuthResponse = r.json().await.unwrap();
                    self.access_token = Some(oauth_response.access_token.clone());
                    println!("Access Token: {}", oauth_response.access_token);
                }
                _ => {
                    println!("Error: Unexpected status code {}", r.status())
                }
            },
            Err(_) => {
                println!("Error: Could not send request to validate session");
            }
        };
    }
}

fn request_id() -> String {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    let ts = now.as_millis().to_string();

    // Extract the last 9 characters.
    let len = ts.len();
    if len > 9 {
        ts[len - 9..].to_string()
    } else {
        ts
    }
}
