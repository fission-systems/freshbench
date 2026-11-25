(() => {
  const button = document.querySelector("[data-theme-toggle]");
  if (!button) return;
  const root = document.documentElement;
  const preferred = () => matchMedia("(prefers-color-scheme: light)").matches ? "light" : "dark";
  let saved;
  try { saved = localStorage.getItem("freshbench-theme"); } catch {}
  if (saved === "light" || saved === "dark") root.dataset.theme = saved;
  function label() {
    const theme = root.dataset.theme || preferred();
    button.textContent = theme === "dark" ? "Light mode" : "Dark mode";
  }
  button.hidden = false;
  label();
  button.addEventListener("click", () => {
    const theme = (root.dataset.theme || preferred()) === "dark" ? "light" : "dark";
    root.dataset.theme = theme;
    try { localStorage.setItem("freshbench-theme", theme); } catch {}
    label();
  });
  matchMedia("(prefers-color-scheme: light)").addEventListener("change", label);
})();
