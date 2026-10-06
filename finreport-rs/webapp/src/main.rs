use actix_files::NamedFile;
use actix_web::web;
use actix_web::{get, Error, HttpResponse, Responder};
use actix_web::{App, HttpServer};
use async_graphql::http::{playground_source, GraphQLPlaygroundConfig};
use dotenv::dotenv;
use secrecy::ExposeSecret;
use std::sync::Arc;
use utils::settings::Settings;
use webapp::db::seaql;
use webapp::graphql::{create_schema, http::cors, http::graphql_handler};

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

#[get("/test-chart")]
async fn test_chart() -> impl Responder {
    tracing::debug!("serving test-chart.json");
    match tokio::fs::read_to_string("../assets/test-chart.json").await {
        Ok(contents) => HttpResponse::Ok()
            .insert_header(("Access-Control-Allow-Origin", "*"))
            .insert_header(("Access-Control-Allow-Methods", "GET"))
            .content_type("application/json")
            .body(contents),
        Err(_) => HttpResponse::NotFound().finish(),
    }
}

async fn playground() -> HttpResponse {
    HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(playground_source(GraphQLPlaygroundConfig::new("/graphql")))
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let app_settings =
        Arc::new(Settings::from_env().expect("Could not load application settings"));

    let database_url = app_settings
        .require_database_url()
        .expect("APP_database_url is required to run webapp");
    let conn = Arc::new(
        seaql::init_db(database_url.expose_secret())
            .await
            .expect("Failed to connect to the database"),
    );

    let schema = create_schema(Arc::clone(&conn), Arc::clone(&app_settings));
    let cors_settings = Arc::clone(&app_settings);
    let handler_settings = Arc::clone(&app_settings);
    HttpServer::new(move || {
        App::new()
            .wrap(cors(&cors_settings))
            .app_data(web::Data::new(schema.clone()))
            .app_data(web::Data::new(Arc::clone(&conn)))
            .app_data(web::Data::new(Arc::clone(&handler_settings)))
            .route("/graphql", web::post().to(graphql_handler))
            .route("/playground", web::get().to(playground))
            .service(root)
            .service(data)
            .service(test_chart)
    })
    .bind(("0.0.0.0", 8080))?
    .run()
    .await
}
