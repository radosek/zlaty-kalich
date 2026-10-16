// Zlatý Kalich browser player: takes the visitor's own copy of the game,
// patches it with the same Rust code as the desktop app (WebAssembly), runs it
// in Ruffle and stands in for the original player.exe (music + saves).
//
//   import { mount } from "/zlaty-kalich/player.js";
//   const player = mount(element, { lang: "cs", onQuit });
//   player.setLang("en"); player.eject();
//
// The bundle is self-contained: every path resolves relative to this file.
// Controls are plain HTML; the host page styles them (boxb uses 98.css).

import { Archive } from "libarchive.js";
import { Music } from "./music.js";
import { AUTOSAVE, db, setting } from "./store.js";

const base = new URL(".", import.meta.url);
const at = (path) => new URL(path, base).href;
const RAR = new TextEncoder().encode("Rar!\x1a\x07");
const STAGE = { w: 800, h: 600 };
/** Stage point of the remote icon that opens the game's menu. */
const MENU_AT = { x: 763, y: 39 };
/** Files we keep from the installer or folder, by lower-case name. */
const WANTED = /^(zlatykalich\.exe|[a-z0-9_]+\.mid|kalich\.sav)$/i;

const TEXT = {
  cs: {
    insert: "Vlož disk",
    intro: "Vyber instalačku 32-ZKinstall.exe, nebo složku se hrou (ZlatyKalich.exe a soubory .mid). Můžeš je sem i přetáhnout.",
    ready: (n) => `Disk je vložený${n ? `, s hudbou (${n} skladeb)` : ", bez hudby: přidej soubory .mid ze složky hry"}.`,
    play: "Hrát",
    browse: "Procházet…",
    folder: "Vybrat složku…",
    notGame: "Tohle nevypadá jako Zlatý Kalich. Vyber instalačku 32-ZKinstall.exe nebo ZlatyKalich.exe.",
    music: "Hudba",
    slot: "Pozice",
    slots: ["automatická", ...[1, 2, 3, 4, 5, 6, 7, 8, 9].map((n) => `pozice ${n}`)],
    hotspots: "Aktivní místa",
    hotspotModes: { "hold-space": "mezerník", always: "vždy", off: "vypnuto" },
    backup: "Zálohovat pozice",
    restore: "Obnovit pozice",
    eject: "Vysunout disk",
    badBackup: "Tohle není záloha pozic ze Zlatého Kalicha.",
    status: ["Hra zůstává jen ve tvém prohlížeči.", "Mezerník: aktivní místa", "Ovladač: páčka = kurzor, A = klik"],
  },
  en: {
    insert: "Insert disc",
    intro: "Pick the installer 32-ZKinstall.exe, or the game folder (ZlatyKalich.exe and the .mid files). You can also drop them here.",
    ready: (n) => `Disc inserted${n ? `, with music (${n} tracks)` : ", no music: add the .mid files from the game folder"}.`,
    play: "Play",
    browse: "Browse…",
    folder: "Choose folder…",
    notGame: "That doesn't look like Zlatý Kalich. Pick the installer 32-ZKinstall.exe or ZlatyKalich.exe.",
    music: "Music",
    slot: "Slot",
    slots: ["autosave", ...[1, 2, 3, 4, 5, 6, 7, 8, 9].map((n) => `slot ${n}`)],
    hotspots: "Hotspots",
    hotspotModes: { "hold-space": "hold Space", always: "always", off: "off" },
    backup: "Back up saves",
    restore: "Restore saves",
    eject: "Eject disc",
    badBackup: "That isn't a Zlatý Kalich save backup.",
    status: ["The game stays in your browser.", "Space: hotspots", "Controller: stick = pointer, A = click"],
  },
};

const STYLE = `
.zk { display: flex; flex-direction: column; height: 100%; min-height: 0; }
.zk-tray { display: grid; grid-template-columns: 120px minmax(0, 1fr); gap: 16px; align-items: center; padding: 16px; }
.zk-tray p { margin: 0 0 10px; max-width: 56ch; }
.zk-tray h4 { margin: 0 0 6px; font-size: 16px; }
.zk-row { display: flex; flex-wrap: wrap; gap: 6px; align-items: center; }
.zk-error { color: #a00000; margin-top: 8px; }
.zk-disc { width: 120px; max-width: 100%; aspect-ratio: 1; border-radius: 50%;
  background: radial-gradient(circle, #c0c0c0 0 10%, #e0e0e0 10.5% 13%, transparent 13.5%),
    conic-gradient(from 20deg, #f3ecdf, #bfd0e2, #f6dc9e, #dcc0de, #c2e3d0, #f3ecdf);
  box-shadow: 0 0 0 1px #808080; }
.zk.is-loading .zk-disc { animation: zk-spin 1.4s linear infinite; }
@keyframes zk-spin { to { transform: rotate(1turn); } }
.zk.is-over .zk-tray { outline: 2px dashed #000080; outline-offset: -6px; }
.zk-bar { display: flex; flex-wrap: wrap; gap: 4px 12px; align-items: center; padding: 4px 6px; }
.zk-bar label { display: inline-flex; gap: 6px; align-items: center; }
.zk-stage { flex: 1; min-height: 240px; background: #000; }
.zk-stage ruffle-player { width: 100%; height: 100%; display: block; }
.zk-cursor { position: fixed; left: 0; top: 0; width: 22px; height: 22px; margin: -11px 0 0 -11px;
  border: 2px solid #ff0; border-radius: 50%; box-shadow: 0 0 0 2px #000; pointer-events: none; z-index: 99999; }
[hidden] { display: none !important; }
@media (max-width: 560px) { .zk-tray { grid-template-columns: 1fr; } }
@media (prefers-reduced-motion: reduce) { .zk-disc { animation: none !important; } }
`;

const MARKUP = `
<div class="zk">
  <div class="zk-tray" data-ref="tray" aria-live="polite">
    <div class="zk-disc" aria-hidden="true"></div>
    <div>
      <h4 data-t="insert"></h4>
      <p data-ref="trayText"></p>
      <div class="zk-row">
        <button type="button" data-ref="play" class="default" hidden data-t="play"></button>
        <button type="button" data-ref="pickFiles" data-t="browse"></button>
        <button type="button" data-ref="pickFolder" data-t="folder"></button>
      </div>
      <p class="zk-error" data-ref="error" hidden></p>
      <input type="file" data-ref="files" multiple hidden accept=".exe,.mid,.sav,.EXE,.MID,.SAV">
      <input type="file" data-ref="folder" webkitdirectory hidden>
    </div>
  </div>
  <div class="zk-bar" data-ref="bar" hidden>
    <label><span data-t="music"></span><input type="range" data-ref="volume" min="0" max="100" step="5"></label>
    <label><span data-t="slot"></span><select data-ref="slot"></select></label>
    <label><span data-t="hotspots"></span><select data-ref="hotspots"></select></label>
    <button type="button" data-ref="backup" data-t="backup"></button>
    <button type="button" data-ref="restore" data-t="restore"></button>
    <button type="button" data-ref="eject" data-t="eject"></button>
    <input type="file" data-ref="restoreFile" accept=".json" hidden>
  </div>
  <div class="zk-stage" data-ref="stage" hidden></div>
  <div class="status-bar" data-ref="status"></div>
</div>`;

function indexOf(haystack, needle) {
  outer: for (let i = 0; i <= haystack.length - needle.length; i++) {
    for (let j = 0; j < needle.length; j++) if (haystack[i + j] !== needle[j]) continue outer;
    return i;
  }
  return -1;
}

let shared;
/** Loads the WebAssembly patcher and libarchive once per page. */
function loadShared() {
  shared ??= (async () => {
    Archive.init({ workerUrl: at("lib/libarchive/worker-bundle.js") });
    const patcher = await import(at("lib/patcher/kalich_web_patcher.js"));
    await patcher.default();
    if ("serviceWorker" in navigator) {
      navigator.serviceWorker.register(at("sw.js"), { type: "module", scope: base.pathname }).catch(console.error);
    }
    return patcher;
  })();
  return shared;
}

export function mount(root, { lang = "cs", onQuit } = {}) {
  if (!document.getElementById("zk-style")) {
    document.head.append(Object.assign(document.createElement("style"), { id: "zk-style", textContent: STYLE }));
  }
  root.innerHTML = MARKUP;
  const el = root.querySelector(".zk");
  const ref = Object.fromEntries([...el.querySelectorAll("[data-ref]")].map((n) => [n.dataset.ref, n]));
  const cursor = Object.assign(document.createElement("div"), { className: "zk-cursor", hidden: true });
  document.body.append(cursor);

  let text = TEXT[lang] ?? TEXT.cs;
  let player = null;
  let music = null;
  let autosaveNext = false;
  let hasGame = false;
  let midiCount = 0;

  function render() {
    for (const n of el.querySelectorAll("[data-t]")) n.textContent = text[n.dataset.t];
    ref.trayText.textContent = hasGame ? text.ready(midiCount) : text.intro;
    ref.status.replaceChildren(
      ...text.status.map((s) => Object.assign(document.createElement("p"), { className: "status-bar-field", textContent: s })),
    );
    fillSelects();
  }

  async function fillSelects() {
    const slot = await setting("slot");
    ref.slot.replaceChildren(...text.slots.map((s, i) => new Option(s, String(i), false, i === slot)));
    const mode = await setting("hotspots");
    ref.hotspots.replaceChildren(
      ...Object.entries(text.hotspotModes).map(([v, s]) => new Option(s, v, false, v === mode)),
    );
  }

  function showError(message) {
    ref.error.textContent = message;
    ref.error.hidden = !message;
  }

  // --- Inserting the game --------------------------------------------------

  async function storeFile(patcher, name, blob) {
    const key = name.toLowerCase();
    if (!WANTED.test(key)) return;
    if (key === "zlatykalich.exe" && !patcher.isGame(new Uint8Array(await blob.arrayBuffer()))) return;
    const d = await db;
    await d.put("files", blob, key);
    // The original install's save becomes slot 1 if there isn't one yet.
    if (key === "kalich.sav" && (await d.get("saves", 1)) == null) await d.put("saves", await blob.text(), 1);
  }

  async function unpackInstaller(patcher, file) {
    const bytes = new Uint8Array(await file.arrayBuffer());
    const start = indexOf(bytes, RAR);
    if (start < 0) return;
    const archive = await Archive.open(new File([bytes.subarray(start)], "game.rar"));
    for (const { file: entry, path } of await archive.getFilesArray()) {
      if (path) continue; // top-level files only
      const extracted = await entry.extract();
      await storeFile(patcher, extracted.name, extracted);
    }
  }

  async function insert(fileList) {
    showError("");
    el.classList.add("is-loading");
    try {
      const patcher = await loadShared();
      for (const file of fileList) {
        const name = file.name.toLowerCase();
        if (name !== "zlatykalich.exe" && name.endsWith(".exe")) await unpackInstaller(patcher, file);
        else await storeFile(patcher, file.name, file);
      }
      await refresh();
      if (!hasGame) showError(text.notGame);
    } catch (e) {
      console.error(e);
      showError(String(e.message ?? e));
    } finally {
      el.classList.remove("is-loading");
    }
  }

  async function refresh() {
    const keys = await (await db).getAllKeys("files");
    hasGame = keys.includes("zlatykalich.exe");
    midiCount = keys.filter((k) => k.endsWith(".mid")).length;
    ref.play.hidden = !hasGame;
    render();
  }

  ref.pickFiles.addEventListener("click", () => ref.files.click());
  ref.pickFolder.addEventListener("click", () => ref.folder.click());
  ref.files.addEventListener("change", (e) => insert([...e.target.files]));
  ref.folder.addEventListener("change", (e) => insert([...e.target.files]));
  ref.tray.addEventListener("dragover", (e) => {
    e.preventDefault();
    el.classList.add("is-over");
  });
  ref.tray.addEventListener("dragleave", () => el.classList.remove("is-over"));
  ref.tray.addEventListener("drop", (e) => {
    e.preventDefault();
    el.classList.remove("is-over");
    insert([...e.dataTransfer.files]);
  });

  // --- Playing -------------------------------------------------------------

  async function soundBank() {
    const res = await fetch("/media/GeneralUser-GS.sf2");
    if (!res.ok) throw new Error(`sound bank: HTTP ${res.status}`);
    return res.arrayBuffer();
  }

  async function onFSCommand(patcher, command, args) {
    const d = await db;
    if (command === "exec") {
      const cmd = patcher.playerExe(args);
      if (cmd.kind === "stop") {
        music?.stop();
      } else if (cmd.kind === "play") {
        const name = cmd.value.toLowerCase();
        const midi = await d.get("files", name);
        if (midi && music) music.play(name, await midi.arrayBuffer());
      } else {
        const slot = autosaveNext ? AUTOSAVE : await setting("slot");
        autosaveNext = false;
        await d.put("saves", cmd.value, slot);
      }
    } else if (command === "zk" && args === "autosave") {
      autosaveNext = true;
    } else if (command === "quit") {
      eject();
      onQuit?.();
    }
    // "fullscreen" and "showmenu" belong to the old projector window.
  }

  async function play() {
    showError("");
    const patcher = await loadShared();
    const d = await db;
    const exe = await d.get("files", "zlatykalich.exe");
    if (!exe) return;
    music ??= new Music(at("lib/spessasynth_processor.min.js"), soundBank, (await setting("volume")) / 100);
    await music.resume();

    const movie = patcher.prepare(new Uint8Array(await exe.arrayBuffer()), await setting("hotspots"), undefined);
    window.RufflePlayer ??= {};
    window.RufflePlayer.config = { publicPath: at("ruffle/") };
    if (!window.RufflePlayer.newest) await import(at("ruffle/ruffle.js"));
    player = window.RufflePlayer.newest().createPlayer();
    player.onFSCommand = (command, args) => {
      onFSCommand(patcher, command, args).catch(console.error);
      return true;
    };
    ref.stage.replaceChildren(player);
    ref.stage.hidden = false;
    ref.tray.hidden = true;
    ref.bar.hidden = false;
    await player.ruffle().load({
      data: movie,
      swfFileName: "kalich.swf",
      base: base.href,
      autoplay: "on",
      unmuteOverlay: "hidden",
      splashScreen: false,
      letterbox: "on",
      quality: "best",
      contextMenu: "off",
      allowFullscreen: false,
    });
    player.focus();
  }

  /** Asks the game to save itself into the autosave slot (F13 -> savegame()). */
  function autosave() {
    if (!player) return;
    for (const type of ["keydown", "keyup"]) {
      window.dispatchEvent(new KeyboardEvent(type, { key: "F13", code: "F13", bubbles: true }));
    }
    autosaveNext = false;
  }

  function eject() {
    autosave();
    music?.stop();
    player?.remove();
    player = null;
    ref.stage.hidden = true;
    ref.tray.hidden = false;
    ref.bar.hidden = true;
  }

  ref.play.addEventListener("click", () => play().catch((e) => showError(String(e.message ?? e))));
  ref.eject.addEventListener("click", eject);
  const onHide = () => document.visibilityState === "hidden" && autosave();
  document.addEventListener("visibilitychange", onHide);
  window.addEventListener("pagehide", autosave);

  // --- Settings ------------------------------------------------------------

  setting("volume").then((v) => (ref.volume.value = v));
  ref.volume.addEventListener("input", async (e) => {
    const v = Number(e.target.value);
    await (await db).put("kv", v, "volume");
    music?.setVolume(v / 100);
  });
  ref.slot.addEventListener("change", async () => (await db).put("kv", Number(ref.slot.value), "slot"));
  ref.hotspots.addEventListener("change", async (e) => {
    const d = await db;
    await d.put("kv", e.target.value, "hotspots");
    if (player) {
      // The highlight is built into the movie: save, then reload from the autosave.
      autosave();
      await new Promise((r) => setTimeout(r, 300));
      await d.put("kv", AUTOSAVE, "slot");
      await fillSelects();
      await play();
    }
  });
  ref.backup.addEventListener("click", async () => {
    const d = await db;
    const keys = await d.getAllKeys("saves");
    const saves = Object.fromEntries(await Promise.all(keys.map(async (k) => [k, await d.get("saves", k)])));
    const blob = new Blob([JSON.stringify({ game: "zlaty-kalich", saves }, null, 1)], { type: "application/json" });
    const a = Object.assign(document.createElement("a"), { href: URL.createObjectURL(blob), download: "zlaty-kalich-saves.json" });
    a.click();
    URL.revokeObjectURL(a.href);
  });
  ref.restore.addEventListener("click", () => ref.restoreFile.click());
  ref.restoreFile.addEventListener("change", async (e) => {
    const data = JSON.parse(await e.target.files[0].text());
    if (data.game !== "zlaty-kalich" || typeof data.saves !== "object") return showError(text.badBackup);
    const d = await db;
    await d.clear("saves");
    for (const [slot, save] of Object.entries(data.saves)) {
      if (typeof save === "string") await d.put("saves", save, Number(slot));
    }
  });

  // --- Controller ----------------------------------------------------------

  const pad = { x: innerWidth / 2, y: innerHeight / 2, a: false, hl: false, start: false, last: performance.now(), raf: 0 };
  const canvas = () => player?.shadowRoot?.querySelector("canvas");
  const pointer = (type, x, y, buttons) =>
    canvas()?.dispatchEvent(new PointerEvent(type, { clientX: x, clientY: y, buttons, button: 0, pointerId: 1, pointerType: "mouse", bubbles: true }));
  const key = (type, code, k) => window.dispatchEvent(new KeyboardEvent(type, { key: k, code, bubbles: true }));
  function stageToClient({ x, y }) {
    const r = canvas()?.getBoundingClientRect();
    if (!r) return null;
    const s = Math.min(r.width / STAGE.w, r.height / STAGE.h);
    return { x: r.left + (r.width - STAGE.w * s) / 2 + x * s, y: r.top + (r.height - STAGE.h * s) / 2 + y * s };
  }
  function pollPad(now) {
    const dt = Math.min((now - pad.last) / 1000, 0.1);
    pad.last = now;
    const gp = [...(navigator.getGamepads?.() ?? [])].find(Boolean);
    cursor.hidden = !(gp && player);
    if (gp && player) {
      let [sx, sy] = [gp.axes[0] ?? 0, gp.axes[1] ?? 0];
      if (gp.buttons[14]?.pressed) sx = -1;
      if (gp.buttons[15]?.pressed) sx = 1;
      if (gp.buttons[12]?.pressed) sy = -1;
      if (gp.buttons[13]?.pressed) sy = 1;
      if (Math.hypot(sx, sy) > 0.18) {
        pad.x = Math.min(innerWidth, Math.max(0, pad.x + sx * 700 * dt));
        pad.y = Math.min(innerHeight, Math.max(0, pad.y + sy * 700 * dt));
        pointer("pointermove", pad.x, pad.y, pad.a ? 1 : 0);
      }
      cursor.style.transform = `translate(${pad.x}px, ${pad.y}px)`;
      const a = Boolean(gp.buttons[0]?.pressed || gp.buttons[7]?.pressed);
      if (a !== pad.a) {
        pad.a = a;
        pointer(a ? "pointerdown" : "pointerup", pad.x, pad.y, a ? 1 : 0);
      }
      const hl = Boolean(gp.buttons[2]?.pressed || gp.buttons[3]?.pressed);
      if (hl !== pad.hl) {
        pad.hl = hl;
        key(hl ? "keydown" : "keyup", "Space", " ");
      }
      const start = Boolean(gp.buttons[9]?.pressed);
      if (start && !pad.start) {
        const p = stageToClient(MENU_AT);
        if (p) {
          pointer("pointermove", p.x, p.y, 0);
          pointer("pointerdown", p.x, p.y, 1);
          pointer("pointerup", p.x, p.y, 0);
        }
      }
      pad.start = start;
    }
    pad.raf = requestAnimationFrame(pollPad);
  }
  pad.raf = requestAnimationFrame(pollPad);

  render();
  refresh();
  loadShared().catch((e) => showError(String(e.message ?? e)));

  return {
    setLang(next) {
      text = TEXT[next] ?? TEXT.cs;
      render();
    },
    eject,
    destroy() {
      eject();
      cancelAnimationFrame(pad.raf);
      cursor.remove();
      document.removeEventListener("visibilitychange", onHide);
      window.removeEventListener("pagehide", autosave);
      root.replaceChildren();
    },
  };
}
