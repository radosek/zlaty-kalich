// IndexedDB storage shared by the page and the service worker.
// files: game files by lower-case name (Blob) · saves: kalich.sav text by
// slot (0 = autosave) · kv: settings.

const DB = "boxb-zlaty-kalich";
const STORES = ["files", "saves", "kv"];

let opening;
function db() {
  opening ??= new Promise((resolve, reject) => {
    const req = indexedDB.open(DB, 1);
    req.onupgradeneeded = () => {
      for (const name of STORES) {
        if (!req.result.objectStoreNames.contains(name)) req.result.createObjectStore(name);
      }
    };
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(req.error);
  });
  return opening;
}

async function run(store, mode, fn) {
  const tx = (await db()).transaction(store, mode);
  const req = fn(tx.objectStore(store));
  return new Promise((resolve, reject) => {
    tx.oncomplete = () => resolve(req?.result);
    tx.onerror = () => reject(tx.error);
    tx.onabort = () => reject(tx.error);
  });
}

export const get = (store, key) => run(store, "readonly", (s) => s.get(key));
export const put = (store, key, value) => run(store, "readwrite", (s) => s.put(value, key));
export const del = (store, key) => run(store, "readwrite", (s) => s.delete(key));
export const clear = (store) => run(store, "readwrite", (s) => s.clear());

export async function entries(store) {
  const tx = (await db()).transaction(store, "readonly");
  const s = tx.objectStore(store);
  const [keys, values] = await Promise.all(
    [s.getAllKeys(), s.getAll()].map(
      (req) => new Promise((resolve, reject) => {
        req.onsuccess = () => resolve(req.result);
        req.onerror = () => reject(req.error);
      }),
    ),
  );
  return keys.map((k, i) => [k, values[i]]);
}

export const AUTOSAVE = 0;
export const DEFAULTS = { slot: 1, volume: 80, hotspots: "hold-space" };

export async function setting(key) {
  return (await get("kv", key)) ?? DEFAULTS[key];
}
