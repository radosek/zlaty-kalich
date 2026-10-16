// Zlatý Kalich in the browser: load the player's own copy, patch it with the
// same Rust code as the desktop app (WebAssembly), run it in Ruffle, and stand
// in for the original player.exe (music + saves).

import initPatcher, { isGame, playerExe, prepare } from "/lib/patcher/kalich_web_patcher.js";
import { Archive } from "/lib/libarchive/libarchive.js";
import { Music } from "/lib/music.js";
import { AUTOSAVE, entries, get, put, clear, setting } from "./store.js";

const $ = (id) => document.getElementById(id);
const tray = $("tray");
const errorBox = $("error");
const RAR = new TextEncoder().encode("Rar!\x1a\x07");
/** Stage point of the remote icon that opens the game's menu. */
const STAGE = { w: 800, h: 600 };

Archive.init({ workerUrl: "/lib/libarchive/worker-bundle.js" });
const patcherReady = initPatcher();
if ("serviceWorker" in navigator) {
  navigator.serviceWorker.register("./sw.js", { type: "module", scope: "./" });
}

const lang = () => document.documentElement.lang;
const t = (cs, en) => (lang() === "cs" ? cs : en);

function showError(message) {
  errorBox.textContent = message;
  errorBox.hidden = !message;
}

// --- Inserting the game ------------------------------------------------

function indexOf(haystack, needle) {
  outer: for (let i = 0; i <= haystack.length - needle.length; i++) {
    for (let j = 0; j < needle.length; j++) if (haystack[i + j] !== needle[j]) continue outer;
    return i;
  }
  return -1;
}

/** Files we keep, by lower-case name. Everything else is ignored. */
const WANTED = /^(zlatykalich\.exe|[a-z0-9_]+\.mid|kalich\.sav)$/i;

async function storeFile(name, blob) {
  const key = name.toLowerCase();
  if (!WANTED.test(key)) return false;
  if (key === "zlatykalich.exe" && !isGame(new Uint8Array(await blob.arrayBuffer()))) return false;
  await put("files", key, blob);
  // The original install's save becomes slot 1 if there isn't one yet.
  if (key === "kalich.sav" && (await get("saves", 1)) == null) {
    await put("saves", 1, await blob.text());
  }
  return true;
}

async function unpackInstaller(file) {
  const bytes = new Uint8Array(await file.arrayBuffer());
  const at = indexOf(bytes, RAR);
  if (at < 0) return 0;
  const archive = await Archive.open(new File([bytes.subarray(at)], "game.rar"));
  let stored = 0;
  for (const { file: entry, path } of await archive.getFilesArray()) {
    if (path) continue; // top-level files only
    const extracted = await entry.extract();
    if (await storeFile(extracted.name, extracted)) stored++;
  }
  return stored;
}

async function insert(fileList) {
  showError("");
  tray.classList.add("is-loading");
  try {
    await patcherReady;
    for (const file of fileList) {
      const name = file.name.toLowerCase();
      if (name !== "zlatykalich.exe" && name.endsWith(".exe")) {
        await unpackInstaller(file);
      } else {
        await storeFile(file.name, file);
      }
    }
    await refreshTray();
    if (!(await get("files", "zlatykalich.exe"))) {
      showError(t("Tohle nevypadá jako Zlatý Kalich. Vyber instalačku nebo ZlatyKalich.exe.",
        "That doesn't look like Zlatý Kalich. Pick the installer or ZlatyKalich.exe."));
    }
  } catch (e) {
    console.error(e);
    showError(String(e.message ?? e));
  } finally {
    tray.classList.remove("is-loading");
  }
}

async function refreshTray() {
  const files = await entries("files");
  const hasGame = files.some(([k]) => k === "zlatykalich.exe");
  const midis = files.filter(([k]) => k.endsWith(".mid")).length;
  $("play").hidden = !hasGame;
  $("tray-text").textContent = hasGame
    ? t(`Disk je vložený${midis ? ` (s hudbou, ${midis} skladeb)` : " (bez hudby — přidej soubory .mid ze složky hry)"}.`,
        `Disc inserted${midis ? ` (with music, ${midis} tracks)` : " (no music — add the .mid files from the game folder)"}.`)
    : $("tray-text").dataset[lang()];
}

$("pick-files").addEventListener("click", () => $("files").click());
$("pick-folder").addEventListener("click", () => $("folder").click());
$("files").addEventListener("change", (e) => insert([...e.target.files]));
$("folder").addEventListener("change", (e) => insert([...e.target.files]));
tray.addEventListener("dragover", (e) => {
  e.preventDefault();
  tray.classList.add("is-over");
});
tray.addEventListener("dragleave", () => tray.classList.remove("is-over"));
tray.addEventListener("drop", (e) => {
  e.preventDefault();
  tray.classList.remove("is-over");
  insert([...e.dataTransfer.files]);
});
document.addEventListener("langchange", refreshTray);

// --- Playing -------------------------------------------------------------

let player = null;
let music = null;
let autosaveNext = false;

async function soundBank() {
  const res = await fetch("/media/GeneralUser-GS.sf2");
  if (!res.ok) throw new Error(`sound bank: ${res.status}`);
  return res.arrayBuffer();
}

async function onFSCommand(command, args) {
  if (command === "exec") {
    const cmd = playerExe(args);
    if (cmd.kind === "stop") {
      music?.stop();
    } else if (cmd.kind === "play") {
      const midi = await get("files", cmd.value.toLowerCase());
      if (midi && music) music.play(cmd.value.toLowerCase(), await midi.arrayBuffer());
    } else {
      const slot = autosaveNext ? AUTOSAVE : await setting("slot");
      autosaveNext = false;
      await put("saves", slot, cmd.value);
    }
  } else if (command === "zk") {
    if (args === "autosave") autosaveNext = true;
  } else if (command === "quit") {
    eject();
    location.href = "/";
  }
  // "fullscreen" and "showmenu" belong to the old projector window.
}

async function play() {
  showError("");
  await patcherReady;
  const exe = await get("files", "zlatykalich.exe");
  if (!exe) return;
  const volume = (await setting("volume")) / 100;
  if (!music) music = new Music("/lib/spessasynth_processor.min.js", soundBank, volume);
  await music.resume();

  const movie = prepare(new Uint8Array(await exe.arrayBuffer()), await setting("hotspots"), undefined);
  window.RufflePlayer = window.RufflePlayer || {};
  window.RufflePlayer.config = { publicPath: "/ruffle/" };
  player = window.RufflePlayer.newest().createPlayer();
  player.onFSCommand = (command, args) => {
    onFSCommand(command, args).catch(console.error);
    return true;
  };
  $("player").replaceChildren(player);
  $("player").hidden = false;
  tray.hidden = true;
  $("toolbar").hidden = false;
  await player.ruffle().load({
    data: movie,
    swfFileName: "kalich.swf",
    base: new URL("./", location.href).href,
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
  $("player").hidden = true;
  tray.hidden = false;
  $("toolbar").hidden = true;
}

$("play").addEventListener("click", () => play().catch((e) => showError(String(e.message ?? e))));
$("eject").addEventListener("click", eject);
document.addEventListener("visibilitychange", () => document.visibilityState === "hidden" && autosave());
window.addEventListener("pagehide", autosave);

// --- Toolbar -------------------------------------------------------------

async function initToolbar() {
  const slot = $("slot");
  const label = () => [t("automatická", "autosave"), ...[1, 2, 3, 4, 5, 6, 7, 8, 9].map((n) => t(`pozice ${n}`, `slot ${n}`))];
  const fill = async () => {
    const current = await setting("slot");
    slot.replaceChildren(...label().map((text, i) => new Option(text, String(i), false, i === current)));
  };
  await fill();
  document.addEventListener("langchange", fill);
  slot.addEventListener("change", () => put("kv", "slot", Number(slot.value)));

  $("volume").value = await setting("volume");
  $("volume").addEventListener("input", (e) => {
    put("kv", "volume", Number(e.target.value));
    music?.setVolume(Number(e.target.value) / 100);
  });

  $("hotspots").value = await setting("hotspots");
  $("hotspots").addEventListener("change", async (e) => {
    await put("kv", "hotspots", e.target.value);
    if (player) {
      // The highlight is built into the movie; reload it from the autosave.
      autosave();
      await new Promise((r) => setTimeout(r, 300));
      await put("kv", "slot", AUTOSAVE);
      await fill();
      await play();
    }
  });

  $("export-saves").addEventListener("click", async () => {
    const saves = Object.fromEntries(await entries("saves"));
    const blob = new Blob([JSON.stringify({ game: "zlaty-kalich", saves }, null, 1)], { type: "application/json" });
    const a = Object.assign(document.createElement("a"), {
      href: URL.createObjectURL(blob),
      download: "zlaty-kalich-saves.json",
    });
    a.click();
    URL.revokeObjectURL(a.href);
  });
  $("import-saves").addEventListener("click", () => $("import-file").click());
  $("import-file").addEventListener("change", async (e) => {
    const data = JSON.parse(await e.target.files[0].text());
    if (data.game !== "zlaty-kalich" || typeof data.saves !== "object") {
      return showError(t("Tohle není záloha pozic ze Zlatého Kalicha.", "That isn't a Zlatý Kalich save backup."));
    }
    await clear("saves");
    for (const [slotKey, text] of Object.entries(data.saves)) {
      if (typeof text === "string") await put("saves", Number(slotKey), text);
    }
  });
}

// --- Controller ----------------------------------------------------------

const pad = { x: innerWidth / 2, y: innerHeight / 2, a: false, x2: false, start: false, last: performance.now() };
const cursor = $("pad-cursor");

function canvas() {
  return player?.shadowRoot?.querySelector("canvas");
}

function pointer(type, x, y, buttons) {
  canvas()?.dispatchEvent(
    new PointerEvent(type, { clientX: x, clientY: y, buttons, button: 0, pointerId: 1, pointerType: "mouse", bubbles: true }),
  );
}

function stageToClient(sx, sy) {
  const rect = canvas()?.getBoundingClientRect();
  if (!rect) return null;
  const scale = Math.min(rect.width / STAGE.w, rect.height / STAGE.h);
  return {
    x: rect.left + (rect.width - STAGE.w * scale) / 2 + sx * scale,
    y: rect.top + (rect.height - STAGE.h * scale) / 2 + sy * scale,
  };
}

function key(type, code, keyName) {
  window.dispatchEvent(new KeyboardEvent(type, { key: keyName, code, bubbles: true }));
}

function pollPad(now) {
  const dt = Math.min((now - pad.last) / 1000, 0.1);
  pad.last = now;
  const gp = [...(navigator.getGamepads?.() ?? [])].find(Boolean);
  if (gp && player) {
    cursor.hidden = false;
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

    const a = gp.buttons[0]?.pressed || gp.buttons[7]?.pressed;
    if (a !== pad.a) {
      pad.a = a;
      pointer(a ? "pointerdown" : "pointerup", pad.x, pad.y, a ? 1 : 0);
    }
    const highlight = gp.buttons[2]?.pressed || gp.buttons[3]?.pressed;
    if (highlight !== pad.x2) {
      pad.x2 = highlight;
      key(highlight ? "keydown" : "keyup", "Space", " ");
    }
    const start = gp.buttons[9]?.pressed;
    if (start && !pad.start) {
      const at = stageToClient(763, 39);
      if (at) {
        pointer("pointermove", at.x, at.y, 0);
        pointer("pointerdown", at.x, at.y, 1);
        pointer("pointerup", at.x, at.y, 0);
      }
    }
    pad.start = start;
  } else {
    cursor.hidden = true;
  }
  requestAnimationFrame(pollPad);
}

requestAnimationFrame(pollPad);
initToolbar();
refreshTray();
