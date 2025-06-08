use std::error::Error;
use std::path::Path;
use tokio::fs;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let schema = webapp::graphql::create_schema();
    let sdl = schema.sdl();
    // Ensure the directory exists
    let dir_path = Path::new("graphql");
    if !dir_path.exists() {
        fs::create_dir_all(dir_path).await?;
    }
    // Write the SDL contents to graphql/schema.graphql
    fs::write("webapp/graphql/schema.graphql", sdl).await?;
    println!("GraphQL schema export written to graphql/schema.graphql");

    Ok(())
}
