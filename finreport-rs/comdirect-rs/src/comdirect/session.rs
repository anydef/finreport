use crate::comdirect::loader;
use crate::comdirect::session_client::{
    Session, SessionClient, SessionClientError, SessionStatus, XOnceAuthenticationInfo,
};
use crate::comdirect::http::build_client;
use reqwest::Error;
use std::fmt::{Display, Formatter};
use std::time::Duration;
use tokio::time::sleep;
use utils::settings::Settings;

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
    fn from(_: SessionClientError) -> Self {
        SessionError::Error
    }
}
impl From<Error> for SessionError {
    fn from(_: Error) -> Self {
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
    SessionReady(Session),
    SessionRefresh(Session),
    Error(SessionClientError),
}

/// Refresh an already-active session using its refresh token. Persists the new
/// access/refresh tokens to the session file so the next bootstrap (or restart)
/// picks them up. Does **not** perform OAuth or the TAN flow.
pub async fn refresh_comdirect_session(
    client_settings: Settings,
    session: &Session,
) -> Result<Session, SessionError> {
    let Settings {
        oauth_url,
        client_id,
        client_secret,
        zugangsnummer,
        pin,
        ..
    } = client_settings.clone();

    let client = build_client();
    let mut comdirect_client = SessionClient::new(
        client_settings.url.clone(),
        oauth_url,
        client_id,
        client_secret,
        zugangsnummer,
        pin,
        client,
    );

    let oauth = comdirect_client.refresh_token_flow(session).await?;
    let new_session = session.refreshed_session(oauth);

    let session_loader = loader::SessionLoader::new(client_settings.save_file_path.to_string());
    if let Err(e) = session_loader.save_session(&new_session).await {
        println!("Warning: failed to persist refreshed session: {:?}", e);
    }

    Ok(new_session)
}

pub async fn load_comdirect_session(client_settings: Settings) -> Result<Session, SessionError> {
    let Settings {
        oauth_url,
        client_id,
        client_secret,
        zugangsnummer,
        pin,
        ..
    } = client_settings;

    let client = build_client();

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
    // The stored session can be 401-expired across runs. Track whether we've
    // already wiped and restarted once so a chronic failure doesn't loop forever.
    let mut already_recovered = false;
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
                    Ok(status) => match status {
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
                    },
                    Err(SessionClientError::Unauthorized) if !already_recovered => {
                        println!(
                            "Stored session rejected (401). Clearing it and restarting from scratch."
                        );
                        session_loader.clear_session().await;
                        already_recovered = true;
                        state = State::NoSession;
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
                // Comdirect's push-TAN flow:
                //   1. Poll GET link.href until the response indicates the user
                //      has approved on their phone (status field flips).
                //   2. Then call patch_session exactly once to activate the session.
                // The PATCH consumes the challenge regardless of state, so we
                // must NOT patch until polling confirms approval.
                let interval = Duration::from_secs(3);
                let max_attempts: u32 = 200; // 3s × 200 = 10 min
                println!(
                    "Approve the push-TAN notification on your phone (polling every {}s, max {} min)...",
                    interval.as_secs(),
                    (interval.as_secs() * max_attempts as u64) / 60
                );

                let mut approved = false;
                let mut last_status: Option<String> = None;
                for _ in 1..=max_attempts {
                    sleep(interval).await;
                    if let Ok(resp) = comdirect_client
                        .get_authentication_status(&session, &auth_info)
                        .await
                    {
                        if last_status.as_deref() != Some(resp.status.as_str()) {
                            println!("Authentication status: {}", resp.status);
                            last_status = Some(resp.status.clone());
                        }
                        if resp.status == "AUTHENTICATED" {
                            approved = true;
                            break;
                        }
                    }
                }

                if !approved {
                    println!("TAN not approved within timeout.");
                    state = State::Error(SessionClientError::Unknown);
                    continue;
                }

                state = match comdirect_client.patch_session(&session, &auth_info).await {
                    Ok(SessionStatus {
                        session_tan_active: true,
                        activated_2fa: true,
                        ..
                    }) => {
                        println!("TAN activated.");
                        State::SessionPatchReady(session)
                    }
                    _ => {
                        println!("patch_session failed after TAN approval.");
                        State::Error(SessionClientError::Unknown)
                    }
                };
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
                        state = State::SessionReady(session.refreshed_session(oauth));
                    }
                    Err(e) if !already_recovered => {
                        println!(
                            "Refresh token rejected ({:?}). Clearing session and restarting.",
                            e
                        );
                        session_loader.clear_session().await;
                        already_recovered = true;
                        state = State::NoSession;
                    }
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
