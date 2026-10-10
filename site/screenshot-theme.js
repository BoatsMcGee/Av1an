// Screenshot thumbnails choose their theme with a `<picture>` media query, but
// an `href` has no media query: the markup's `<a>` is written against the light
// file, so a link would otherwise open the light screenshot next to a dark
// thumbnail.
//
// Point each link at the same `prefers-color-scheme` variant the thumbnail
// shows, on load and whenever the preference changes.
(() => {
  const dark = window.matchMedia("(prefers-color-scheme: dark)");

  const align = () => {
    const [from, to] = dark.matches
      ? ["-light.avif", "-dark.avif"]
      : ["-dark.avif", "-light.avif"];

    for (const link of document.querySelectorAll(`a[href$="${from}"]`)) {
      const href = link.getAttribute("href");
      link.setAttribute("href", href.slice(0, -from.length) + to);
    }
  };

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", align, { once: true });
  } else {
    align();
  }

  dark.addEventListener("change", align);
})();
