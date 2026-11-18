(function () {
  try {
    var saved = localStorage.getItem("qwitch.theme");
    var prefersDark =
      window.matchMedia &&
      window.matchMedia("(prefers-color-scheme: dark)").matches;
    var isDark = saved === "dark" || (!saved && prefersDark);
    document.documentElement.classList.toggle("dark", isDark);
  } catch (_) {}
})();