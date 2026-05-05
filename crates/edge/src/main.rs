use std::env;

use async_trait::async_trait;
use log::info;
use pingora::prelude::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RouteTarget {
    Web,
    Server,
}

#[derive(Default)]
struct RequestContext {
    target: Option<RouteTarget>,
}

struct KodamapubEdge {
    web_addr: String,
    web_host: String,
    upstream_addr: String,
    upstream_host: String,
}

impl KodamapubEdge {
    fn from_env() -> Self {
        let web_addr = env::var("UPSTREAM_WEB_ADDR").unwrap_or_else(|_| "web:5173".to_string());
        let web_host = env::var("UPSTREAM_WEB_HOST").unwrap_or_else(|_| "web".to_string());
        let upstream_addr = env::var("UPSTREAM_ADDR").unwrap_or_else(|_| "server:3000".to_string());
        let upstream_host = env::var("UPSTREAM_HOST").unwrap_or_else(|_| "server".to_string());

        Self {
            web_addr,
            web_host,
            upstream_addr,
            upstream_host,
        }
    }

    fn route_target(&self, session: &Session) -> RouteTarget {
        let request = session.req_header();
        let path = request.uri.path();
        let method = request.method.as_str();

        if matches!(method, "POST" | "PUT" | "PATCH" | "DELETE") {
            return RouteTarget::Server;
        }

        if path == "/health" || path.starts_with("/.well-known/") {
            return RouteTarget::Server;
        }

        if path.starts_with("/users/") || path.starts_with("/posts/") {
            if accepts_json(request) {
                return RouteTarget::Server;
            }
        }

        RouteTarget::Web
    }
}

#[async_trait]
impl ProxyHttp for KodamapubEdge {
    type CTX = RequestContext;

    fn new_ctx(&self) -> Self::CTX {
        RequestContext::default()
    }

    async fn upstream_peer(
        &self,
        session: &mut Session,
        ctx: &mut Self::CTX,
    ) -> Result<Box<HttpPeer>> {
        let target = self.route_target(session);
        ctx.target = Some(target);

        let (addr, host) = match target {
            RouteTarget::Web => (&self.web_addr, &self.web_host),
            RouteTarget::Server => (&self.upstream_addr, &self.upstream_host),
        };

        let peer = HttpPeer::new(addr.as_str(), false, host.clone());
        Ok(Box::new(peer))
    }

    async fn upstream_request_filter(
        &self,
        session: &mut Session,
        upstream_request: &mut RequestHeader,
        ctx: &mut Self::CTX,
    ) -> Result<()> {
        upstream_request
            .insert_header("x-forwarded-proto", "http")
            .ok();

        upstream_request
            .insert_header("x-kodamapub-edge", "pingora")
            .ok();

        if let Some(target) = ctx.target {
            upstream_request
                .insert_header(
                    "x-kodamapub-route",
                    match target {
                        RouteTarget::Web => "web",
                        RouteTarget::Server => "server",
                    },
                )
                .ok();
        }

        if let Some(host) = session
            .req_header()
            .headers
            .get("host")
            .and_then(|value| std::str::from_utf8(value.as_bytes()).ok())
        {
            upstream_request.insert_header("x-forwarded-host", host).ok();
        }

        Ok(())
    }

    async fn logging(&self, session: &mut Session, error: Option<&Error>, ctx: &mut Self::CTX) {
        let status = session
            .response_written()
            .map_or(0, |response| response.status.as_u16());
        let request = session.request_summary();
        let client = session
            .client_addr()
            .map(|addr| addr.to_string())
            .unwrap_or_else(|| "-".to_string());
        let error = error.map(ToString::to_string).unwrap_or_else(|| "-".to_string());
        let target = ctx.target.map_or("unknown", |target| match target {
            RouteTarget::Web => "web",
            RouteTarget::Server => "server",
        });

        info!(
            target: "access",
            "client={} upstream={} status={} request={} error={}",
            client,
            target,
            status,
            request,
            error
        );
    }
}

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info"),
    )
    .format_timestamp_secs()
    .init();

    let listen_addr = env::var("EDGE_LISTEN_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".to_string());

    let proxy = KodamapubEdge::from_env();

    let mut server = Server::new(None)?;
    server.bootstrap();

    let mut service = http_proxy_service(&server.configuration, proxy);
    service.add_tcp(&listen_addr);
    server.add_service(service);

    info!("kodamapub-edge started listen_addr={}", listen_addr);
    server.run_forever();
}

fn accepts_json(request: &RequestHeader) -> bool {
    request
        .headers
        .get("accept")
        .and_then(|value| std::str::from_utf8(value.as_bytes()).ok())
        .is_some_and(|value| value.contains("application/json"))
}
