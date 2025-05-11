use std::fmt::{Display, Formatter};
use std::time::Duration;
use reqwest::Error;
use tokio::io;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::time::sleep;
use crate::comdirect::loader;
use crate::comdirect::session_client::{SessionClientError, SessionClient, Session, SessionStatus, XOnceAuthenticationInfo};
use crate::settings::Settings;

#[derive(Debug)]
pub enum SessionError {
    Error,
}

impl Display for SessionError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "Client error: {:?}", self)
    }
}

impl From<SessionClientError> for SessionError {
    fn from(value: SessionClientError) -> Self {
        SessionError::Error
    }
}
impl From<reqwest::Error> for SessionError {
    fn from(value: Error) -> Self {
        SessionError::Error
    }
}

impl std::error::Error for SessionError {}

enum State {
    Start,
    NoSession,
    SessionUnchecked(Session),
    SessionValidationReady(Session),
    SessionPatchReady(Session),
    SessionPatchWaitingForTan(Session, XOnceAuthenticationInfo),
    SessionPatchSession(Session, XOnceAuthenticationInfo),
    SessionReady(Session),
    SessionRefresh(Session),
    Error(SessionClientError),
}

pub async fn get_comdirect_session(client_settings: Settings) -> Result<Session, SessionError> {
    let Settings {
        oauth_url,
        client_id,
        client_secret,
        zugangsnummer,
        pin,
        ..
    } = client_settings;

    let client = reqwest::Client::builder()
        .connection_verbose(false)
        .build()?;

    let mut comdirect_client = SessionClient::new(
        client_settings.url,
        oauth_url,
        client_id,
        client_secret,
        zugangsnummer,
        pin,
        client,
    );

    let session_loader = loader::SessionLoader::new(client_settings.save_file_path.to_string());
    let mut state = State::Start;
    let session_result = loop {
        match state {
            State::Start => {
                println!("Starting session...");
                let session_result = session_loader.load_session().await;
                match session_result {
                    Some(session) => {
                        state = State::SessionUnchecked(session);
                    }
                    None => {
                        state = State::NoSession;
                    }
                }
            }
            State::NoSession => {
                println!("No session found, creating a new one.");
                let oauth = comdirect_client.acquire_password_token().await?;
                state = State::SessionUnchecked(Session::from_oauth(oauth))
            }

            State::SessionUnchecked(session) => {
                println!("Session unchecked. Checking status...");
                let status = comdirect_client.get_session_status(&session).await;
                match status {
                    Ok(status) => {
                        match status {
                            SessionStatus {
                                identifier,
                                session_tan_active: true,
                                activated_2fa: true,
                            } => {
                                state = State::SessionRefresh(Session {
                                    access_token: session.access_token,
                                    refresh_token: session.refresh_token,
                                    session_uuid: identifier,
                                });
                            }
                            SessionStatus {
                                identifier,
                                session_tan_active: false,
                                activated_2fa: false,
                            } => {
                                state = State::SessionValidationReady(Session {
                                    access_token: session.access_token,
                                    refresh_token: session.refresh_token,
                                    session_uuid: identifier,
                                });
                            }
                            _ => {
                                println!("Unknown session state.");
                                state = State::NoSession;
                            }
                        }
                    }
                    Err(e) => {
                        println!("Error getting session status: {:?}", e);
                        state = State::Error(SessionClientError::Unknown);
                    }
                }

            }
            State::SessionValidationReady(session) => {
                //validate session POST
                println!("Validating session...");
                let auth_info = comdirect_client.validate_session(&session).await?;
                state = State::SessionPatchWaitingForTan(session, auth_info);
            }

            State::SessionPatchWaitingForTan(session, auth_info) => {
                // wait a minute for the user to enter the TAN
                println!("Waiting for TAN...");
                println!("Press Enter to continue...");

                let mut stdin = BufReader::new(io::stdin());
                let mut line = String::new();
                tokio::select! {
                            _ = stdin.read_line(&mut line) => {
                                println!("Enter pressed. Continuing execution.");
                            }
                            _ = sleep(Duration::from_secs(599)) => {
                                println!("5 Minutes timeout reached. Continuing execution.");
                            }
                }

                // Wait for the user to type a line and press Enter
                state = State::SessionPatchSession(session,auth_info);
            }

            State::SessionPatchSession(session, auth_info) => {
                //validate session PATCH
                let patched_session = comdirect_client.patch_session(&session, &auth_info).await?;
                match patched_session {
                    SessionStatus {
                        identifier: _identifier,
                        session_tan_active: true,
                        activated_2fa: true,
                    } => {
                        println!("Session is valid.");
                        state = State::SessionPatchReady(session);
                    }
                    _ => {
                        println!("Session validation failed.");
                        state = State::Error(SessionClientError::Unknown);
                    }
                }
            }
            State::SessionPatchReady(session) => {
                //activate cd secondary flow
                let session = comdirect_client.activate_secondary_flow(&session).await?;
                state = State::SessionReady(session);
            }
            State::SessionRefresh(session) => {
                let oauth_result = comdirect_client.refresh_token_flow(&session).await;
                match oauth_result {
                    Ok(oauth) => {
                        println!("Session refreshed successfully.");
                        state = State::SessionReady(session.refreshed_session(oauth));                    }
                    Err(e) => {
                        println!("Error refreshing session: {:?}", e);
                        state = State::Error(SessionClientError::Unknown);
                    }
                }
            }

            State::SessionReady(session) => {
                let result = session_loader.save_session(&session).await;
                if let Err(e) = result {
                    println!("Error saving session: {:?}", e);
                    state = State::Error(SessionClientError::Unknown);
                } else {
                    break Ok(session);
                }
            }
            State::Error(e) => {
                session_loader.clear_session().await;
                println!("Error occurred: {:?}", e);
                break Err(e);
            }
        };
    };

    Ok(session_result?)
}