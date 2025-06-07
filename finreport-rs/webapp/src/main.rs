use actix_files::NamedFile;
use actix_web::{get, Error, HttpResponse, Responder};

use actix_files as fs;
use actix_web::{App, HttpServer};

#[get("/")]
async fn root() -> Result<NamedFile, Error> {
    Ok(NamedFile::open("../assets/index.html")?)
}

#[get("/data")]
async fn data() -> impl Responder {
    match tokio::fs::read_to_string("../assets/data.json").await {
        Ok(contents) => HttpResponse::Ok()
            .insert_header(("Access-Control-Allow-Origin", "*"))
            .insert_header(("Access-Control-Allow-Methods", "GET"))
            .content_type("application/json")
            .body(contents),
        Err(_) => HttpResponse::NotFound().finish(),
    }
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    HttpServer::new(|| {
        App::new()
            .service(root)
            .service(data)
            .service(fs::Files::new("/assets", ".").show_files_listing())
    })
    .bind(("127.0.0.1", 8080))?
    .run()
    .await
}
