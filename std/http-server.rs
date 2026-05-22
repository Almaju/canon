#[derive(Clone)]
pub struct OnewayHttpRouter(axum::Router);

#[allow(dead_code)]
fn oneway_http_router_new() -> OnewayHttpRouter {
    OnewayHttpRouter(axum::Router::new())
}

#[allow(dead_code)]
fn oneway_http_get_route(
    router: OnewayHttpRouter,
    route_path: RoutePath,
    handler: fn(HttpRequest) -> HttpResponse,
) -> OnewayHttpRouter {
    OnewayHttpRouter(router.0.route(
        &route_path.0,
        axum::routing::get(move || async move { handler(HttpRequest(String::new())).0 }),
    ))
}

#[allow(dead_code)]
fn oneway_http_post_route(
    router: OnewayHttpRouter,
    route_path: RoutePath,
    handler: fn(HttpRequest) -> HttpResponse,
) -> OnewayHttpRouter {
    OnewayHttpRouter(router.0.route(
        &route_path.0,
        axum::routing::post(move |body: String| async move { handler(HttpRequest(body)).0 }),
    ))
}

#[allow(dead_code)]
async fn oneway_http_serve(router: OnewayHttpRouter, port: Port) -> Result<(), std::io::Error> {
    let addr = format!("0.0.0.0:{}", port.0);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, router.0).await
}
