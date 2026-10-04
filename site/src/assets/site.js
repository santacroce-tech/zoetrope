// Fills the download buttons from the latest GitHub release and highlights
// the visitor's platform. Without JavaScript or if the API is unreachable,
// the buttons simply point at the releases page.
(() => {
  const REPO = "santacroce-tech/zoetrope";

  const os = (() => {
    const p = (navigator.userAgentData?.platform || navigator.platform || navigator.userAgent).toLowerCase();
    if (p.includes("mac")) return "mac";
    if (p.includes("win")) return "windows";
    if (p.includes("linux") || p.includes("x11")) return "linux";
    return null;
  })();
  const NAMES = { mac: "macOS", windows: "Windows", linux: "Linux" };

  /** Which release files go under which platform, in display order. */
  const KINDS = [
    { os: "mac", test: /\.dmg$/i, label: "Disk image (.dmg)" },
    { os: "windows", test: /setup\.exe$/i, label: "Installer (.exe)" },
    { os: "windows", test: /\.msi$/i, label: "MSI package" },
    { os: "linux", test: /\.AppImage$/i, label: "AppImage" },
    { os: "linux", test: /\.deb$/i, label: ".deb (Debian/Ubuntu)" },
    { os: "linux", test: /\.rpm$/i, label: ".rpm (Fedora)" },
  ];

  const mark = () => {
    if (!os) return;
    document.querySelector(`.platform[data-os="${os}"]`)?.classList.add("yours");
    const hero = document.getElementById("hero-download");
    if (hero) hero.textContent = `Download for ${NAMES[os]}`;
  };

  const fill = (release) => {
    const byOs = {};
    for (const k of KINDS) {
      const asset = release.assets.find((a) => k.test.test(a.name));
      if (asset) (byOs[k.os] ??= []).push({ ...k, url: asset.browser_download_url, size: asset.size });
    }
    for (const [key, list] of Object.entries(byOs)) {
      const box = document.querySelector(`[data-assets="${key}"]`);
      if (!box) continue;
      box.replaceChildren(
        ...list.map((a, i) => {
          const link = document.createElement("a");
          link.className = i === 0 ? "button primary" : "button";
          link.href = a.url;
          link.textContent = a.label;
          link.title = `${(a.size / 1048576).toFixed(1)} MB`;
          return link;
        }),
      );
    }
    const hero = document.getElementById("hero-download");
    if (hero && os && byOs[os]) hero.href = byOs[os][0].url;
    const line = document.getElementById("release-line");
    if (line) {
      const date = new Date(release.published_at).toLocaleDateString(undefined, { year: "numeric", month: "long", day: "numeric" });
      line.textContent = `Version ${release.tag_name.replace(/^v/, "")}, released ${date}. Free, open source, no account needed.`;
    }
  };

  document.addEventListener("DOMContentLoaded", () => {
    mark();
    if (!document.querySelector("[data-assets]")) return;
    fetch(`https://api.github.com/repos/${REPO}/releases/latest`, { headers: { Accept: "application/vnd.github+json" } })
      .then((r) => (r.ok ? r.json() : Promise.reject(r.status)))
      .then(fill)
      .catch(() => {});
  });
})();
