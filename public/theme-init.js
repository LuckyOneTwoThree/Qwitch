(function () {
  try {
    var params = new URLSearchParams(window.location.search);
    if (params.get("view") === "tray") {
      document.documentElement.classList.add("tray-window");
    }
    var saved = localStorage.getItem("qwitch.theme");
    var prefersDark =
      window.matchMedia &&
      window.matchMedia("(prefers-color-scheme: dark)").matches;
    var isDark =
      saved === "dark" || ((saved === "system" || !saved) && prefersDark);
    document.documentElement.classList.toggle("dark", isDark);
  } catch (_) {}
})();