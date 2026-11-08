// SPDX-License-Identifier: EPL-2.0

//! `HttpGithub` against a loopback stand-in for GitHub: the requests it makes (paths, our
//! credentials, the token in the body) and how it reads the answers. The router's tests use
//! `FakeGithub`; this is the real client they stand in for.

// The loopback HTTP server of rumble-client's tests.
#[path = "../../../crates/rumble-client/tests/support/mod.rs"]
mod support;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use rumble_lambda::github::{Github, GithubError, HttpGithub};
use serde_json::json;
use support::{Req, Resp, Server};

/// GitHub's API and web hosts, both on `server`; calls run as the Lambda runs them: on a
/// thread inside the runtime's context.
fn with_github<T: Send + 'static>(
    server: &Server,
    call: impl FnOnce(&HttpGithub) -> T + Send + 'static,
) -> T {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let github = {
        let _context = runtime.enter();
        HttpGithub::with_hosts("app-id", "app-secret", &server.url(""), &server.url("/"))
    };
    let task = runtime.spawn_blocking(move || call(&github));
    runtime.block_on(task).unwrap()
}

fn basic(req: &Req) -> String {
    let value = req.header("authorization").unwrap_or_default();
    let encoded = value.strip_prefix("Basic ").unwrap_or_default();
    String::from_utf8(STANDARD.decode(encoded).unwrap()).unwrap()
}

#[test]
fn check_token_asks_the_application_api_with_our_credentials() {
    let server = Server::start(|req| match req.path.as_str() {
        "/applications/app-id/token" => Resp::json(200, &json!({"user": {"id": 4711}})),
        _ => Resp::status(500),
    });
    assert_eq!(with_github(&server, |g| g.check_token("gho_x")), Ok(4711));
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    let req = &requests[0];
    assert_eq!(req.method, "POST");
    assert_eq!(basic(req), "app-id:app-secret");
    let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
    assert_eq!(body, json!({"access_token": "gho_x"}));
    assert_eq!(req.header("x-github-api-version"), Some("2022-11-28"));
}

#[test]
fn a_token_of_another_app_or_without_a_user_is_rejected() {
    // GitHub answers 404 for a token our app didn't issue, 422 for a malformed one.
    for status in [404, 422, 401] {
        let server = Server::start(move |_| Resp::json(status, &json!({"message": "Not Found"})));
        assert!(
            matches!(
                with_github(&server, |g| g.check_token("t")),
                Err(GithubError::Rejected(_))
            ),
            "{status}"
        );
    }
    let server = Server::start(|_| Resp::json(200, &json!({"user": null})));
    assert!(matches!(
        with_github(&server, |g| g.check_token("t")),
        Err(GithubError::Rejected(_))
    ));
    // GitHub down is not a rejection: the caller can retry.
    let server = Server::start(|_| Resp::status(502));
    assert!(matches!(
        with_github(&server, |g| g.check_token("t")),
        Err(GithubError::Unavailable(_))
    ));
}

#[test]
fn exchange_code_posts_the_code_and_reads_the_token_or_the_error() {
    let server = Server::start(|req| match req.path.as_str() {
        "/login/oauth/access_token" if req.body_text().contains("code=good") => Resp::json(
            200,
            &json!({"access_token": "gho_new", "token_type": "bearer"}),
        ),
        // GitHub answers 200 with an error for a bad or used code.
        _ => Resp::json(
            200,
            &json!({"error": "bad_verification_code", "error_description": "The code is wrong."}),
        ),
    });
    assert_eq!(
        with_github(&server, |g| g.exchange_code("good", "https://site/cb")),
        Ok("gho_new".to_string())
    );
    assert_eq!(
        with_github(&server, |g| g.exchange_code("used", "https://site/cb")),
        Err(GithubError::Rejected("The code is wrong.".into()))
    );
    let form = server.requests()[0].body_text();
    for part in [
        "client_id=app-id",
        "client_secret=app-secret",
        "code=good",
        "redirect_uri=https%3A%2F%2Fsite%2Fcb",
    ] {
        assert!(form.contains(part), "{form}");
    }
    assert_eq!(
        server.requests()[0].header("accept"),
        Some("application/json")
    );
}

#[test]
fn user_reads_the_profile_with_the_users_token() {
    let server = Server::start(
        |req| match (req.path.as_str(), req.header("authorization")) {
            ("/user", Some("Bearer gho_x")) => Resp::json(
                200,
                &json!({"id": 7, "login": "ann", "name": "", "created_at": "2011-01-25T18:44:36Z"}),
            ),
            _ => Resp::status(401),
        },
    );
    let profile = with_github(&server, |g| g.user("gho_x")).unwrap();
    assert_eq!((profile.id, profile.login.as_str()), (7, "ann"));
    assert_eq!(profile.name, None, "an empty name is no name");
    assert_eq!(profile.created_at, Some(1_295_981_076));
    assert!(matches!(
        with_github(&server, |g| g.user("other")),
        Err(GithubError::Rejected(_))
    ));
}

#[test]
fn delete_token_and_user_id() {
    let server = Server::start(|req| match (req.method.as_str(), req.path.as_str()) {
        ("DELETE", "/applications/app-id/token") => Resp::status(204),
        ("GET", "/users/ann") => Resp::json(200, &json!({"id": 7, "login": "ann"})),
        _ => Resp::status(404),
    });
    assert_eq!(with_github(&server, |g| g.delete_token("gho_x")), Ok(()));
    assert_eq!(with_github(&server, |g| g.user_id("ann")), Ok(Some(7)));
    assert_eq!(with_github(&server, |g| g.user_id("nobody")), Ok(None));
    let asked = server.requests().len();
    // Not a login: no request at all (nothing reaches the path).
    for login in ["", "../user", "a b", "ann/repos"] {
        assert_eq!(with_github(&server, move |g| g.user_id(login)), Ok(None));
    }
    assert_eq!(server.requests().len(), asked);
    assert_eq!(basic(&server.requests()[0]), "app-id:app-secret");
}
