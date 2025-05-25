use finreport::categorize::{CategorizeAiResponse, Category};
use finreport::comdirect::transaction::Transaction;
use finreport::db::{Persistence, DB_URL};
use rig::{completion::Prompt, providers::openai};
use serde_json::json;
use tokio::fs;
use tokio::io::{AsyncBufReadExt, BufReader};

#[tokio::main]
async fn main() {
    let key = finreport::categorize::settings()
        .await
        .expect("Failed to load settings")
        .openai_key;
    let openai_client = openai::Client::new(key.as_str());
    let categories_json = fs::read_to_string("prompts/categories.json")
        .await
        .expect("Failed to read categories");

    let prompt_template: String = fs::read_to_string("prompts/categorize.txt")
        .await
        .expect("Failed to read systemd prompt");

    let updated_prompt = prompt_template.replace("{categories}", &categories_json);
    let gpt4 = openai_client
        .agent("o4-mini")
        // .agent("gpt-4.1")
        .preamble(updated_prompt.as_str())
        .additional_params(json!({
            "response_format": { "type": "json_object" }
        }))
        .build();

    let mut transactions: Vec<String> = Vec::new();

    let file = fs::File::open("transactions-102455031500.ndjson")
        .await
        .expect("Failed to open file");
    let reader = BufReader::new(file);
    let mut lines = reader.lines();

    while let Some(line) = lines.next_line().await.expect("Failed to read line") {
        if line.trim().is_empty() {
            continue;
        }
        transactions.push(line);
    }
    let mut persistence = Persistence::new(DB_URL)
        .await
        .expect("Failed to create persistence");

    let categories = serde_json::from_str::<Vec<Category>>(&categories_json)
        .expect("Failed to parse categories");
    persistence
        .load_categories(&categories)
        .await
        .expect("Failed to init categories");

    for t in transactions.iter() {
        let transaction =
            serde_json::from_str::<Transaction>(&t).expect("Failed to parse transaction");
        let is_categorized: bool = persistence
            .check_categorized(&transaction)
            .await
            .expect("Failed to check if transaction is categorized");
        if is_categorized {
            println!("Transaction already categorized: {t}");
            continue;
        } else {
            // println!("Not categorized: {t}");
            let response = gpt4
                .prompt(t)
                // .prompt(&transactions[random_transaction])
                .await
                .expect("Failed to prompt GPT-4");

            let r: CategorizeAiResponse = serde_json::from_str(response.as_str()).unwrap();
            println!("GPT-4 o4-mini. Transaction: {r}");
            persistence
                .add_category(&transaction, &r)
                .await
                .expect("Failed to add category to transaction");
        }
    }
    // let random_transaction = rand::rng().random_range(0..transactions.len());
    // // Prompt the model and print its response
    // let response = gpt4
    //     .prompt(&transactions[273])
    //     // .prompt(&transactions[random_transaction])
    //     .await
    //     .expect("Failed to prompt GPT-4");
    //
    // let r: CategorizeAiResponse = serde_json::from_str(response.as_str()).unwrap();
    // println!("GPT-4 o4-mini. Transaction {random_transaction}: {r}");
}
