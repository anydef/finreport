use dotenv::dotenv;
use comdirect_rs::comdirect::session::get_comdirect_session;
use utils::settings::Settings;
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

    println!("Session status: {:?}", session_result);

    Ok(())
}
