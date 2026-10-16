// A tiny Windows 98 window manager: icons, windows, taskbar, clock.
// Windows are elements with class "window" and a data-title; icons open the
// window named in data-open (or follow their href).

const tasks = document.querySelector(".tasks");
let z = 10;

export function activate(win) {
  for (const w of document.querySelectorAll(".window")) w.classList.toggle("is-active", w === win);
  win.style.zIndex = String(++z);
  for (const t of tasks?.children ?? []) t.classList.toggle("is-active", t.dataset.for === win.id);
}

export function open(win) {
  win.hidden = false;
  if (!tasks.querySelector(`[data-for="${win.id}"]`)) {
    const task = document.createElement("button");
    task.className = "task out";
    task.type = "button";
    task.dataset.for = win.id;
    task.textContent = win.dataset.title;
    task.addEventListener("click", () => {
      if (win.hidden) win.hidden = false;
      activate(win);
    });
    tasks.append(task);
  }
  activate(win);
  win.querySelector(".default, button, a")?.focus({ preventScroll: true });
}

export function close(win) {
  win.hidden = true;
  tasks.querySelector(`[data-for="${win.id}"]`)?.remove();
  win.dispatchEvent(new Event("close"));
}

function wire(win) {
  win.addEventListener("pointerdown", () => activate(win));
  win.querySelector(".ctl .close")?.addEventListener("click", () => close(win));
  win.querySelector(".ctl .min")?.addEventListener("click", () => {
    win.hidden = true;
    tasks.querySelector(`[data-for="${win.id}"]`)?.classList.remove("is-active");
  });
  win.querySelector(".ctl .max")?.addEventListener("click", () => win.classList.toggle("is-max"));

  const bar = win.querySelector(".titlebar");
  bar.addEventListener("dblclick", () => win.classList.toggle("is-max"));
  bar.addEventListener("pointerdown", (e) => {
    if (e.target.closest("button") || win.classList.contains("is-max") || innerWidth <= 640) return;
    const rect = win.getBoundingClientRect();
    const dx = e.clientX - rect.left;
    const dy = e.clientY - rect.top;
    win.style.transform = "none";
    win.style.left = `${rect.left}px`;
    win.style.top = `${rect.top}px`;
    bar.setPointerCapture(e.pointerId);
    const move = (ev) => {
      win.style.left = `${Math.max(-rect.width + 80, Math.min(innerWidth - 80, ev.clientX - dx))}px`;
      win.style.top = `${Math.max(0, Math.min(innerHeight - 60, ev.clientY - dy))}px`;
    };
    const up = () => {
      bar.removeEventListener("pointermove", move);
      bar.removeEventListener("pointerup", up);
    };
    bar.addEventListener("pointermove", move);
    bar.addEventListener("pointerup", up);
  });
}

for (const win of document.querySelectorAll(".window")) wire(win);

// Icons: click selects, double-click (or Enter, or a tap on touch screens) opens.
const icons = document.querySelectorAll(".icon");
for (const icon of icons) {
  const go = () => {
    if (icon.dataset.open) open(document.getElementById(icon.dataset.open));
    else if (icon.dataset.href) location.href = icon.dataset.href;
  };
  icon.addEventListener("click", (e) => {
    for (const other of icons) other.classList.toggle("is-sel", other === icon);
    if (e.pointerType === "touch" || matchMedia("(pointer: coarse)").matches) go();
  });
  icon.addEventListener("dblclick", go);
  icon.addEventListener("keydown", (e) => e.key === "Enter" && go());
}

const clock = document.getElementById("clock");
function tick() {
  const d = new Date();
  clock.textContent = `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
}
if (clock) {
  tick();
  setInterval(tick, 20000);
}
