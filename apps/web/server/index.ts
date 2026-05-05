import { serve } from "@hono/node-server";
import { serveStatic } from "@hono/node-server/serve-static";
import { Hono } from "hono";

const app = new Hono();
const apiOrigin = process.env.API_ORIGIN ?? "http://server:3000";
const port = Number(process.env.PORT ?? "5173");

app.use("*", async (c, next) => {
  await next();

  c.header("x-content-type-options", "nosniff");
  c.header("x-frame-options", "DENY");
  c.header("referrer-policy", "same-origin");
  c.header("permissions-policy", "camera=(), microphone=(), geolocation=()");
  c.header(
    "content-security-policy",
    "default-src 'self'; img-src 'self' data: https:; style-src 'self' 'unsafe-inline'; script-src 'self'; connect-src 'self'; frame-ancestors 'none'",
  );
});

function wantsActivityPub(request: Request): boolean {
  const accept = request.headers.get("accept") ?? "";
  return accept.includes("application/activity+json") || accept.includes("application/ld+json");
}

function copyProxyHeaders(upstream: Response): Headers {
  const headers = new Headers();

  for (const name of [
    "content-type",
    "cache-control",
    "etag",
    "last-modified",
    "content-length",
    "set-cookie",
  ]) {
    const value = upstream.headers.get(name);
    if (value) {
      headers.set(name, value);
    }
  }

  return headers;
}

async function proxyToApi(request: Request, path: string): Promise<Response> {
  const headers: Record<string, string> = {
    accept: request.headers.get("accept") ?? "application/json",
    "content-type": request.headers.get("content-type") ?? "application/json",
    cookie: request.headers.get("cookie") ?? "",
  };

  for (const name of [
    "origin",
    "referer",
    "x-csrf-token",
    "x-forwarded-for",
    "x-forwarded-host",
    "x-forwarded-proto",
    "host",
  ]) {
    const value = request.headers.get(name);
    if (value) {
      headers[name] = value;
    }
  }

  const upstream = await fetch(`${apiOrigin}${path}`, {
    method: request.method,
    headers,
    body:
      request.method === "GET" || request.method === "HEAD"
        ? undefined
        : await request.arrayBuffer(),
  });

  return new Response(upstream.body, {
    status: upstream.status,
    headers: copyProxyHeaders(upstream),
  });
}

app.get("/health", (c) => c.json({ status: "ok", service: "web" }));

app.get("/.well-known/webfinger", (c) => {
  const search = new URL(c.req.url).search;
  return proxyToApi(c.req.raw, `/api/.well-known/webfinger${search}`);
});

app.get("/api/session", (c) => proxyToApi(c.req.raw, "/api/session"));

app.post("/api/login", (c) => proxyToApi(c.req.raw, "/api/login"));

app.post("/api/logout", (c) => proxyToApi(c.req.raw, "/api/logout"));

app.on(["GET", "POST"], "/users/:username/posts", async (c) => {
  const username = encodeURIComponent(c.req.param("username"));
  const query = c.req.url.includes("?") ? c.req.url.slice(c.req.url.indexOf("?")) : "";
  return proxyToApi(c.req.raw, `/api/users/${username}/posts${query}`);
});

app.post("/users/:username/inbox", async (c) => {
  const username = encodeURIComponent(c.req.param("username"));
  return proxyToApi(c.req.raw, `/api/users/${username}/inbox`);
});

app.get("/users/:username/outbox", async (c) => {
  const username = encodeURIComponent(c.req.param("username"));
  const query = c.req.url.includes("?") ? c.req.url.slice(c.req.url.indexOf("?")) : "";
  return proxyToApi(c.req.raw, `/api/users/${username}/outbox${query}`);
});

app.get("/users/:username", async (c) => {
  const username = encodeURIComponent(c.req.param("username"));
  return proxyToApi(c.req.raw, `/api/users/${username}`);
});

app.get("/posts/:postId", async (c, next) => {
  if (!wantsActivityPub(c.req.raw)) {
    await next();
    return;
  }

  const postId = encodeURIComponent(c.req.param("postId"));
  return proxyToApi(c.req.raw, `/api/posts/${postId}`);
});

app.use(
  "/assets/*",
  serveStatic({
    root: "./dist",
  }),
);

app.get("/favicon.ico", serveStatic({ path: "./dist/favicon.ico" }));
app.get("*", serveStatic({ path: "./dist/index.html" }));

serve({
  fetch: app.fetch,
  port,
});
