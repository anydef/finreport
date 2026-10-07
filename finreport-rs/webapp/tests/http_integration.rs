//! HTTP-level integration test (§8/§9): round-trips the `login` mutation's
//! `Set-Cookie` through actix's `test::call_service`, then uses that cookie
//! to prove `me` resolves back to the same user — exercising the exact
//! `graphql::http` wiring `main.rs` runs, not a stand-in for it.
#![cfg(feature = "integration")]

mod common;

use actix_web::{test, web, App};
use webapp::graphql::{create_schema, http::cors, http::graphql_resource};

#[actix_web::test]
async fn login_cookie_round_trips_and_identifies_the_caller() {
    let db = common::db().await;
    let (_user_id, username) = common::seed_user(&db, "erin-http-test", "s3cret-password").await;

    let settings = common::dummy_settings();
    let schema = create_schema(db.clone(), settings.clone());

    let app = test::init_service(
        App::new()
            .wrap(cors(&settings))
            .app_data(web::Data::new(schema))
            .app_data(web::Data::new(db))
            .app_data(web::Data::new(settings.clone()))
            .service(graphql_resource()),
    )
    .await;

    // Unauthenticated `me` is `null` (§5), not an error.
    let me_before_req = test::TestRequest::post()
        .uri("/graphql")
        .set_json(serde_json::json!({ "query": "{ me { username } }" }))
        .to_request();
    let me_before: serde_json::Value = test::call_and_read_body_json(&app, me_before_req).await;
    assert_eq!(me_before["data"]["me"], serde_json::Value::Null);

    // Login: the response must carry a `Set-Cookie: fr_session=...`.
    let login_req = test::TestRequest::post()
        .uri("/graphql")
        .set_json(serde_json::json!({
            "query": "mutation($u: String!, $p: String!) { login(input: { username: $u, password: $p }) { username } }",
            "variables": { "u": username, "p": "s3cret-password" }
        }))
        .to_request();
    let login_resp = test::call_service(&app, login_req).await;
    assert!(login_resp.status().is_success(), "login did not succeed: {login_resp:?}");

    let set_cookie = login_resp
        .headers()
        .get("set-cookie")
        .expect("login must set fr_session cookie")
        .to_str()
        .unwrap()
        .to_string();
    assert!(set_cookie.starts_with("fr_session="), "{set_cookie}");

    let cookie_value = set_cookie.split(';').next().unwrap().to_string();

    // Wrong credentials: same generic error, no user enumeration.
    let bad_login_req = test::TestRequest::post()
        .uri("/graphql")
        .set_json(serde_json::json!({
            "query": "mutation($u: String!, $p: String!) { login(input: { username: $u, password: $p }) { username } }",
            "variables": { "u": username, "p": "wrong-password" }
        }))
        .to_request();
    let bad_login: serde_json::Value = test::call_and_read_body_json(&app, bad_login_req).await;
    assert_eq!(
        bad_login["errors"][0]["extensions"]["code"],
        "INVALID_CREDENTIALS"
    );

    // The cookie from the successful login now authenticates `me`.
    let me_req = test::TestRequest::post()
        .uri("/graphql")
        .insert_header(("cookie", cookie_value.clone()))
        .set_json(serde_json::json!({ "query": "{ me { username } }" }))
        .to_request();
    let me_after: serde_json::Value = test::call_and_read_body_json(&app, me_req).await;
    assert_eq!(me_after["data"]["me"]["username"], username.as_str());

    // `logout` clears the cookie; the old token stops authenticating.
    let logout_req = test::TestRequest::post()
        .uri("/graphql")
        .insert_header(("cookie", cookie_value.clone()))
        .set_json(serde_json::json!({ "query": "mutation { logout }" }))
        .to_request();
    let logout_resp = test::call_service(&app, logout_req).await;
    assert!(logout_resp.status().is_success());

    let me_after_logout_req = test::TestRequest::post()
        .uri("/graphql")
        .insert_header(("cookie", cookie_value))
        .set_json(serde_json::json!({ "query": "{ me { username } }" }))
        .to_request();
    let me_after_logout: serde_json::Value =
        test::call_and_read_body_json(&app, me_after_logout_req).await;
    assert_eq!(me_after_logout["data"]["me"], serde_json::Value::Null);
}

/// §4 CSRF defense in depth: a POST to `/graphql` whose `Content-Type` isn't
/// `application/json` must be rejected before it ever reaches the resolver,
/// not merely fail GraphQL parsing — async-graphql's own body parser treats
/// any non-multipart content type as JSON, so without the dedicated guard
/// this request would otherwise succeed.
#[actix_web::test]
async fn non_json_content_type_is_rejected_with_415() {
    let db = common::db().await;
    let settings = common::dummy_settings();
    let schema = create_schema(db.clone(), settings.clone());

    let app = test::init_service(
        App::new()
            .wrap(cors(&settings))
            .app_data(web::Data::new(schema))
            .app_data(web::Data::new(db))
            .app_data(web::Data::new(settings.clone()))
            .service(graphql_resource()),
    )
    .await;

    for content_type in ["text/plain", "application/x-www-form-urlencoded"] {
        let req = test::TestRequest::post()
            .uri("/graphql")
            .insert_header(("content-type", content_type))
            .set_payload(r#"{"query":"{ me { username } }"}"#)
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(
            resp.status(),
            actix_web::http::StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "expected 415 for Content-Type: {content_type}, got {:?}",
            resp.status()
        );
    }

    // `application/json; charset=utf-8` (what browsers/fetch commonly send)
    // must still pass — the guard only ignores parameters, not the whole
    // header.
    let ok_req = test::TestRequest::post()
        .uri("/graphql")
        .insert_header(("content-type", "application/json; charset=utf-8"))
        .set_payload(r#"{"query":"{ me { username } }"}"#)
        .to_request();
    let ok_resp: serde_json::Value = test::call_and_read_body_json(&app, ok_req).await;
    assert_eq!(ok_resp["data"]["me"], serde_json::Value::Null);
}
