import { serve } from '@hono/node-server';
import { serveStatic } from '@hono/node-server/serve-static';
import { Hono } from 'hono';

const app = new Hono();
const apiOrigin = process.env.API_ORIGIN ?? 'http://server:3000';
const port = Number(process.env.PORT ?? '5173');

function copyProxyHeaders(upstream: Response): Headers {
  const headers = new Headers();

  for (const name of [
    'content-type',
    'cache-control',
    'etag',
    'last-modified',
    'content-length',
  ]) {
    const value = upstream.headers.get(name);
    if (value) {
      headers.set(name, value);
    }
  }

  return headers;
}

async function proxyToApi(request: Request, path: string): Promise<Response> {
  const upstream = await fetch(`${apiOrigin}${path}`, {
    method: request.method,
    headers: {
      accept: request.headers.get('accept') ?? 'application/json',
      'content-type': request.headers.get('content-type') ?? 'application/json',
    },
    body:
      request.method === 'GET' || request.method === 'HEAD'
        ? undefined
        : await request.arrayBuffer(),
  });

  return new Response(upstream.body, {
    status: upstream.status,
    headers: copyProxyHeaders(upstream),
  });
}

app.get('/health', (c) => c.json({ status: 'ok', service: 'web' }));

app.on(['GET', 'POST'], '/users/:username/posts', async (c) => {
  const username = encodeURIComponent(c.req.param('username'));
  const query = c.req.url.includes('?') ? c.req.url.slice(c.req.url.indexOf('?')) : '';
  return proxyToApi(c.req.raw, `/api/users/${username}/posts${query}`);
});

app.get('/users/:username', async (c) => {
  const username = encodeURIComponent(c.req.param('username'));
  return proxyToApi(c.req.raw, `/api/users/${username}`);
});

app.use(
  '/assets/*',
  serveStatic({
    root: './dist',
  }),
);

app.get('/favicon.ico', serveStatic({ path: './dist/favicon.ico' }));
app.get('*', serveStatic({ path: './dist/index.html' }));

serve({
  fetch: app.fetch,
  port,
});
