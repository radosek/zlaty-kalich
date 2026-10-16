// boxb.org: static files from Workers Static Assets; the 32 MB sound bank is
// over the 25 MiB asset limit, so it is served from R2.
export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    if (url.pathname.startsWith("/media/")) {
      const key = url.pathname.slice("/media/".length);
      const object = await env.MEDIA.get(key);
      if (!object) return new Response("Not found", { status: 404 });
      const headers = new Headers();
      object.writeHttpMetadata(headers);
      headers.set("etag", object.httpEtag);
      headers.set("cache-control", "public, max-age=31536000, immutable");
      return new Response(object.body, { headers });
    }
    return env.ASSETS.fetch(request);
  },
};
