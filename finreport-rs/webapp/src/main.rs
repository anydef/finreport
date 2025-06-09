use actix_cors::Cors;
use actix_files as fs;
use actix_files::NamedFile;
use actix_web::web;
use actix_web::{get, Error, HttpResponse, Responder};
use actix_web::{App, HttpServer};
use async_graphql::http::{playground_source, GraphQLPlaygroundConfig};
use async_graphql_actix_web::{GraphQLRequest, GraphQLResponse};
use dotenv::dotenv;
use sea_orm::{Database, DatabaseConnection};
use std::sync::Arc;
use utils::settings::Settings;
use webapp::graphql::{create_schema, AppSchema};
use migration::{Migrator, MigratorTrait};
use webapp::db::seaql;

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
    println!("test-chart.json");
    match tokio::fs::read_to_string("../assets/test-chart.json").await {
        Ok(contents) => HttpResponse::Ok()
            .insert_header(("Access-Control-Allow-Origin", "*"))
            .insert_header(("Access-Control-Allow-Methods", "GET"))
            .content_type("application/json")
            .body(contents),
        Err(_) => HttpResponse::NotFound().finish(),
    }
}

async fn graphql_handler(schema: web::Data<AppSchema>, req: GraphQLRequest) -> GraphQLResponse {
    schema.execute(req.into_inner()).await.into()
}

async fn playground() -> HttpResponse {
    HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(playground_source(GraphQLPlaygroundConfig::new("/graphql")))
}

struct AppState {
    conn: DatabaseConnection,
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    dotenv().ok();
    env_logger::init();

    let config = config::Config::builder()
        .add_source(
            config::Environment::with_prefix("APP")
                .prefix_separator("_")
                .separator("__"),
        )
        .build()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

    let app_settings = Arc::new(
        config
            .try_deserialize::<Settings>()
            .expect("Could not load application settings"),
    );

    let conn = seaql::init_db(&app_settings.database_url).await;
    // let app_settings_clone = Arc::clone(&app_settings);

    // // refresh session every minute
    // tokio::spawn(async move {
    //     // let settings = app_settings_clone.clone();
    //     let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(60));
    //
    //     loop {
    //         let _ = load_comdirect_session((*app_settings).clone()).await;
    //         interval.tick().await;
    //         // Place the code to execute every minute here
    //         println!("Running scheduled task every minute");
    //     }
    // });

    let schema = create_schema();
    HttpServer::new(move || {
        App::new()
            .wrap(
                Cors::default()
                    .allow_any_origin()
                    .allowed_methods(vec!["GET", "POST"])
                    .allow_any_header(),
            )
            .app_data(web::Data::new(schema.clone()))
            .route("/graphql", web::post().to(graphql_handler))
            .route("/playground", web::get().to(playground))
            .service(root)
            .service(data)
            .service(test_chart)
            .service(fs::Files::new("/assets", ".").show_files_listing())
    })
    .bind(("127.0.0.1", 8080))?
    .run()
    .await
}
