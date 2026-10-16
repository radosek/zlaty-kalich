// Answers the movie's loadVariables("kalich.sav") from the selected save slot,
// so saving and loading work like the original, all inside the browser.
import { db, setting } from "./store.js";

self.addEventListener("install", () => self.skipWaiting());
self.addEventListener("activate", (event) => event.waitUntil(self.clients.claim()));

self.addEventListener("fetch", (event) => {
  const url = new URL(event.request.url);
  if (url.origin !== location.origin || !/\/kalich\.sav$/i.test(url.pathname)) return;
  event.respondWith(
    (async () => {
      const save = await (await db).get("saves", await setting("slot"));
      return save == null
        ? new Response("", { status: 404 })
        : new Response(save, { headers: { "content-type": "text/plain; charset=windows-1250" } });
    })(),
  );
});
