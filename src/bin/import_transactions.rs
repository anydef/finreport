use dotenv::dotenv;
use finreport::comdirect::session::get_comdirect_session;
use finreport::settings::Settings;
use std::env;
use std::error::Error;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    dotenv().ok();
    unsafe {
        env::set_var("RUST_LOG", "reqwest=trace");
    }
    env_logger::init();

    let settings = config::Config::builder()
        .add_source(
            config::Environment::with_prefix("APP")
                .prefix_separator("_")
                .separator("__"),
        )
        .build()?;
    let client_settings = settings
        .try_deserialize::<Settings>()
        .expect("Could not load application settings");

    let session_result = get_comdirect_session(client_settings).await;



    Ok(())
}

// #[tokio::main]
// async fn main() -> Result<(), Box<dyn Error>> {
//     dotenv().ok();
//     unsafe {
//         env::set_var("RUST_LOG", "reqwest=trace");
//     }
//     env_logger::init();
//
//     let settings = config::Config::builder()
//         .add_source(
//             config::Environment::with_prefix("APP")
//                 .prefix_separator("_")
//                 .separator("__"),
//         )
//         .build()?;
//
//     let oauth_settings = settings
//         .try_deserialize::<Settings>()
//         .expect("Could not load application settings");
//
//     let params = [
//         ("client_id", oauth_settings.client_id.clone()),
//         ("client_secret", oauth_settings.client_secret.clone()),
//         ("username", oauth_settings.zugangsnummer.clone()),
//         ("password", oauth_settings.pin.clone()),
//         ("grant_type", "password".to_string()),
//     ];
//
//     let mut headers = HeaderMap::new();
//     headers.insert(
//         "Content-Type",
//         "application/x-www-form-urlencoded".parse().unwrap(),
//     );
//     headers.insert("Accept", "application/json".parse().unwrap());
//
//     let client = reqwest::Client::builder()
//         .connection_verbose(false)
//         .default_headers(headers)
//         .build()?;
//
//     let oauth_response = client
//         .post(format!("{}/oauth/token", oauth_settings.oauth_url))
//         .form(&params)
//         .send()
//         .await?;
//     let oauth_response: OAuthResponse = match oauth_response.json().await {
//         Ok(response) => response,
//         Err(_) => {
//             println!("Error: Could not parse JSON response");
//             return Ok(());
//         }
//     };
//
//     println!("Access Token: {}", oauth_response.access_token);
//     let session_id = Uuid::new_v4().to_string();
//     let info_header = HttpRequestInfoHeader::from(session_id.clone(), request_id());
//     let info_header_json = serde_json::to_string(&info_header).unwrap();
//     let session_status_response = client
//         .get(format!(
//             "{}/session/clients/user/v1/sessions",
//             oauth_settings.url
//         ))
//         .header(
//             "Authorization",
//             format!("Bearer {}", oauth_response.access_token),
//         )
//         .header("x-http-request-info", info_header_json.clone())
//         .send()
//         .await?;
//
//     let session_status: Vec<SessionStatus> = match session_status_response.json().await {
//         Ok(response) => response,
//         Err(e) => {
//             println!("Error: Could not parse JSON response: {:}", e);
//             return Ok(());
//         }
//     };
//     let current_session = session_status.first().unwrap();
//
//     if !current_session.activated_2fa || !current_session.session_tan_active {
//         let patched = SessionStatus {
//             identifier: current_session.identifier.clone(),
//             session_tan_active: true,
//             activated_2fa: true,
//         };
//
//         let r = client
//             .post(format!(
//                 "{}/session/clients/user/v1/sessions/{}/validate",
//                 oauth_settings.url,
//                 current_session.identifier.clone()
//             ))
//             .json(&patched)
//             // .header("Content-Type", "application/json")
//             .header("x-http-request-info", info_header_json.clone())
//             .header(
//                 "Authorization",
//                 format!("Bearer {}", oauth_response.access_token),
//             )
//             .send()
//             .await?;
//
//         println!("Patched session {}", r.status());
//         let auth_info: GenericResult<AuthenticationInfo> = match r.status() {
//             StatusCode::CREATED => {
//                 println!("Session patched successfully");
//                 let authentication_info: AuthenticationInfo =
//                     match r.headers().get("x-once-authentication-info") {
//                         Some(header) => {
//                             let header_str = header.to_str().unwrap_or_default();
//                             let authentication_info: AuthenticationInfo =
//                                 serde_json::from_str(header_str).unwrap();
//                             authentication_info
//                         }
//                         None => {
//                             println!("Error: Missing x-once-authentication-info header");
//                             return Ok(());
//                         }
//                     };
//                 println!("Authentication Info: {:?}", authentication_info);
//                 Ok(authentication_info)
//             }
//             _ => {
//                 println!("Error: Unexpected status code {}", r.status());
//                 Err(GenericError)
//             }
//         };
//         println!("Waiting for TAN to be approved...");
//         println!("Press ENTER to continue...");
//         let stdin = io::stdin();
//         let mut reader = BufReader::new(stdin);
//         let mut input = String::new();
//         reader.read_line(&mut input).await?;
//         println!("You entered: {}", input.trim());
//
//         let mut challenge = HashMap::new();
//         challenge.insert("id", auth_info.unwrap().challenge_id);
//         let patched_session = client
//             .patch(format!(
//                 "{}/session/clients/user/v1/sessions/{}",
//                 oauth_settings.url,
//                 current_session.identifier.clone()
//             ))
//             .json(&patched)
//             .header(
//                 "Authorization",
//                 format!("Bearer {}", oauth_response.access_token),
//             )
//             .header(
//                 "x-once-authentication-info",
//                 serde_json::to_string(&challenge).expect("Failed to serialize challenge"),
//             )
//             .header("x-http-request-info", info_header_json.clone())
//             .header("x-once-authentication", "000000")
//             .send()
//             .await?;
//
//         println!("Patched sessions status {}", patched_session.status());
//         let params = [
//             ("client_id", oauth_settings.client_id.clone()),
//             ("client_secret", oauth_settings.client_secret.clone()),
//             ("token", oauth_response.access_token.clone()),
//             ("grant_type", "cd_secondary".to_string()),
//         ];
//
//         let oauth_cd_secondary_flow = client
//             .post(format!("{}/oauth/token", oauth_settings.oauth_url))
//             .header("Content-Type", "application/x-www-form-urlencoded")
//             .header("Accept", "application/json")
//             .form(&params)
//             .send()
//             .await?;
//
//         println!(
//             "OAuth CD Secondary Flow: {}",
//             oauth_cd_secondary_flow.status()
//         );
//     }
//
//     let accounts_balances = client
//         .get(format!(
//             "{}/banking/clients/user/v2/accounts/balances",
//             oauth_settings.url
//         ))
//         .header(
//             "Authorization",
//             format!("Bearer {}", oauth_response.access_token.clone()),
//         )
//         .header("x-http-request-info", info_header_json.clone())
//         .send()
//         .await?;
//     println!("Accounts Balances: {}", accounts_balances.status());
//
//     Ok(())
// }
//
// fn request_id() -> String {
//     let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
//     let ts = now.as_millis().to_string();
//
//     // Extract the last 9 characters.
//     let len = ts.len();
//     if len > 9 {
//         ts[len - 9..].to_string()
//     } else {
//         ts
//     }
// }
//
// // fn aaa() -> Session {
// //     let session = Session {};
// //     session
// // }
