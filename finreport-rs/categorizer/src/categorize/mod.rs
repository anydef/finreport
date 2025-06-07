use dotenv::dotenv;
use serde::Deserialize;
use std::error::Error;
use std::fmt::{Display, Formatter};

#[derive(Deserialize, Debug, Clone)]
pub struct Settings {
    pub openai_key: String,
}

pub async fn settings() -> Result<Settings, Box<dyn Error>> {
    dotenv().ok();
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

    Ok(client_settings)
}

#[derive(Deserialize, Debug, Clone)]
pub struct Category {
    pub category: String,
    pub subcategories: Vec<String>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct CategorizeAiResponse {
    pub reference: String,
    pub category: String,
    pub subcategory: String,
    pub confidence: f32,
    pub reasoning: String,
}

impl Display for CategorizeAiResponse {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Reference: {}, Category: {}, Subcategory: {}, Confidence: {}, Reasoning: {}",
            self.reference, self.category, self.subcategory, self.confidence, self.reasoning
        )
    }
}
