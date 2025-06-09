use actix_files::NamedFile;
use actix_web::{get, Error, HttpResponse, Responder};
use actix_cors::Cors;
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
use actix_web::{web};
use async_graphql::http::{playground_source, GraphQLPlaygroundConfig};
use async_graphql_actix_web::{GraphQLRequest, GraphQLResponse};
use webapp::graphql::{create_schema, AppSchema};

async fn graphql_handler(
    schema: web::Data<AppSchema>,
    req: GraphQLRequest,
) -> GraphQLResponse {
    schema.execute(req.into_inner()).await.into()
}

async fn playground() -> HttpResponse {
    HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(playground_source(GraphQLPlaygroundConfig::new("/graphql")))
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {

    tokio::spawn(async {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(60));

        loop {
            interval.tick().await;
            // Place the code to execute every minute here
            println!("Running scheduled task every minute");
        }
    });

    let schema = create_schema();
    HttpServer::new(move || {
        App::new()
            .wrap(
                Cors::default()
                    .allow_any_origin()
                    .allowed_methods(vec!["GET", "POST"])
                    .allow_any_header()
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
