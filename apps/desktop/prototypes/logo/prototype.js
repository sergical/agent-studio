const logo = {
  src: "../../public/skill-studio-logo.png",
  alt: "Skill Studio logo",
};

const stage = document.getElementById("stage");

function logoImage(size) {
  return `<img src="${logo.src}" alt="${logo.alt}" width="${size}" height="${size}" />`;
}

stage.innerHTML = `
  <article class="logo-sheet">
    <header>
      <p>Final logo</p>
      <h1>Skill Studio</h1>
    </header>

    <section class="hero-preview" aria-label="Logo preview">
      ${logoImage(224)}
      <strong>Skill Studio</strong>
    </section>

    <section class="surface-grid" aria-label="Logo surface tests">
      <div class="surface surface-dark">
        ${logoImage(124)}
        <span>Dark</span>
      </div>
      <div class="surface surface-light">
        ${logoImage(124)}
        <span>Light</span>
      </div>
    </section>

    <section class="size-tests" aria-label="Logo size tests">
      <div>${logoImage(124)}<span>124 px</span></div>
      <div>${logoImage(48)}<span>48 px</span></div>
      <div>${logoImage(16)}<span>16 px</span></div>
    </section>
  </article>
`;
