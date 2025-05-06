use tokio::io;
use tokio::io::{AsyncBufReadExt, BufReader};

pub async fn wait_user_input() -> Result<usize, io::Error> {

    let stdin = io::stdin();
    let mut reader = BufReader::new(stdin);
    let mut input = String::new();
    println!("Approve TAN and press ENTER to continue...");
    let result = reader.read_line(&mut input).await?;
    println!("You entered: {}", input.trim());
    Ok(result)
}