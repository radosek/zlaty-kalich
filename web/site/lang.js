// Czech / English for static text: elements carry data-cs and data-en.
const KEY = "boxb-lang";

export function currentLang() {
  try {
    const saved = localStorage.getItem(KEY);
    if (saved === "cs" || saved === "en") return saved;
  } catch {}
  return navigator.language?.toLowerCase().startsWith("cs") ||
    navigator.language?.toLowerCase().startsWith("sk")
    ? "cs"
    : "en";
}

export function applyLang(lang) {
  document.documentElement.lang = lang;
  for (const el of document.querySelectorAll("[data-cs]")) {
    el.textContent = el.dataset[lang];
  }
  for (const button of document.querySelectorAll("[data-lang-toggle]")) {
    button.textContent = lang === "cs" ? "EN" : "CZ";
  }
  document.dispatchEvent(new CustomEvent("langchange", { detail: lang }));
}

let lang = currentLang();
applyLang(lang);
for (const button of document.querySelectorAll("[data-lang-toggle]")) {
  button.addEventListener("click", () => {
    lang = lang === "cs" ? "en" : "cs";
    try {
      localStorage.setItem(KEY, lang);
    } catch {}
    applyLang(lang);
  });
}
