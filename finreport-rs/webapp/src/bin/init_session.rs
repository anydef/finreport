use dotenv::dotenv;
use comdirect_rs::comdirect::session::load_comdirect_session;
use utils::settings::Settings;
use std::env;
use std::error::Error;
use std::time::Duration;
use tokio::time::sleep;

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
    loop {
        let session_result = load_comdirect_session(client_settings.clone()).await;
        println!("Session status: {:?}", session_result);
        sleep(Duration::from_secs(300)).await;
    }


}
