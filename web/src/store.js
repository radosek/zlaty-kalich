// Browser storage shared by the player and its service worker (via idb).
// files: game files by lower-case name · saves: kalich.sav text by slot
// (0 = autosave) · kv: settings.
import { openDB } from "idb";

export const AUTOSAVE = 0;
export const DEFAULTS = { slot: 1, volume: 80, hotspots: "hold-space" };

export const db = openDB("boxb-zlaty-kalich", 1, {
  upgrade(d) {
    for (const name of ["files", "saves", "kv"]) {
      if (!d.objectStoreNames.contains(name)) d.createObjectStore(name);
    }
  },
});

export async function setting(key) {
  return (await (await db).get("kv", key)) ?? DEFAULTS[key];
}
