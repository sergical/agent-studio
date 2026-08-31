const variants = [
  {
    name: "Current",
    eyebrow: "Baseline",
    description: "The existing two-part orbit mark. It reads as balance or exchange, but it does not point clearly to skills, tools, or a studio.",
    cost: "Friendly, but generic and hard to connect to the product.",
    asset: "../../src-tauri/icons/icon.png",
    alt: "Current Skill Studio logo with cyan and yellow circular forms",
  },
  {
    name: "Modules",
    eyebrow: "Reusable building blocks",
    description: "An S assembled from discrete skill modules. The separated pieces suggest skills that can move between agent environments.",
    cost: "The clearest small icon, but the most restrained personality.",
    asset: "./variants/modules.svg",
    alt: "An S made from four violet rounded modules",
  },
  {
    name: "Relay",
    eyebrow: "One skill, many agents",
    description: "A checked skill passes through two routes. It puts Skill Studio's main job, keeping one capability usable across agents, directly into the mark.",
    cost: "The strongest product story, but it carries more detail at 16 pixels.",
    asset: "./variants/relay.svg",
    alt: "Two violet routes passing through a checked skill block",
  },
  {
    name: "Studio press",
    eyebrow: "A toolmaker's stamp",
    description: "A compact S monogram cut into a hexagonal seal. It treats Skill Studio as a serious workshop for making and maintaining agent capabilities.",
    cost: "The most ownable shape, but less literal about sync and agents.",
    asset: "./variants/studio-press.svg",
    alt: "A violet hexagonal seal containing a geometric S",
  },
];

const stage = document.getElementById("stage");
const picker = document.querySelector(".proto-picker");
const highlight = picker.querySelector(".proto-picker-highlight");
const items = [...picker.querySelectorAll(".proto-picker-item")];
let current = 0;

function logoImage(variant, className = "") {
  return `<img class="logo ${className}" src="${variant.asset}" alt="${variant.alt}" width="192" height="192" />`;
}

function renderVariant(variant) {
  return `
    <article class="concept">
      <header class="concept-header">
        <p class="eyebrow">${variant.eyebrow}</p>
        <h1>${variant.name}</h1>
        <p class="description">${variant.description}</p>
      </header>

      <section class="hero-preview" aria-label="Primary logo preview">
        <div class="hero-mark">${logoImage(variant)}</div>
        <div class="wordmark">
          <span>Skill</span>
          <span>Studio</span>
        </div>
      </section>

      <section class="context-grid" aria-label="Logo usage previews">
        <div class="preview-card dark-card">
          <div class="preview-label">macOS app icon</div>
          <div class="app-icon">${logoImage(variant, "app-icon-mark")}</div>
        </div>
        <div class="preview-card light-card">
          <div class="preview-label">Sidebar lockup</div>
          <div class="sidebar-mock">
            <div class="window-dots" aria-hidden="true"><i></i><i></i><i></i></div>
            <div class="small-lockup">${logoImage(variant, "small-mark")}<strong>Skill Studio</strong></div>
            <div class="fake-nav"><span></span><span></span><span></span></div>
          </div>
        </div>
        <div class="preview-card neutral-card">
          <div class="preview-label">Small-size test</div>
          <div class="size-row">
            <div>${logoImage(variant, "size-32")}<span>32</span></div>
            <div>${logoImage(variant, "size-24")}<span>24</span></div>
            <div>${logoImage(variant, "size-16")}<span>16</span></div>
          </div>
          <div class="mono-test">${logoImage(variant, "mono-mark")}<span>One-color read</span></div>
        </div>
      </section>

      <p class="cost"><span>Tradeoff</span>${variant.cost}</p>
    </article>
  `;
}

function moveHighlight() {
  const el = items[current];
  highlight.style.width = `${el.offsetWidth}px`;
  highlight.style.transform = `translateX(${el.offsetLeft}px)`;
}

function mount(i) {
  stage.innerHTML = renderVariant(variants[i]);
}

function setActive(i) {
  if (i < 0 || i >= variants.length) return;
  current = i;
  items.forEach((el, j) => {
    el.toggleAttribute("data-active", j === i);
    if (j === i) el.setAttribute("aria-current", "true");
    else el.removeAttribute("aria-current");
  });
  moveHighlight();
  const url = new URL(location);
  url.searchParams.set("v", i + 1);
  history.replaceState(null, "", url);
  mount(i);
}

items.forEach((el, i) => el.addEventListener("click", () => setActive(i)));
window.addEventListener("resize", moveHighlight);

document.addEventListener("keydown", (event) => {
  if (/^(INPUT|TEXTAREA|SELECT)$/.test(event.target.tagName) || event.target.isContentEditable) return;
  if (event.metaKey || event.ctrlKey || event.altKey) return;
  const num = Number.parseInt(event.key, 10);
  if (num >= 1 && num <= variants.length) setActive(num - 1);
  else if (event.key === "ArrowRight") setActive((current + 1) % variants.length);
  else if (event.key === "ArrowLeft") setActive((current - 1 + variants.length) % variants.length);
});

setActive((Number.parseInt(new URLSearchParams(location.search).get("v"), 10) || 1) - 1);
requestAnimationFrame(() => requestAnimationFrame(() => picker.setAttribute("data-ready", "")));
