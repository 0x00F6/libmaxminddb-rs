// Link each worker-scaling curve to its legend in inline report SVGs.
for (const chart of document.querySelectorAll("svg.lookup-api-chart")) {
  const series = Array.from(chart.querySelectorAll(".api-series"));
  let selected = null;
  let hovered = null;

  function render() {
    const active = hovered || selected;
    for (const item of series) {
      item.classList.toggle("is-active", item === active);
      item.classList.toggle("is-muted", active !== null && item !== active);
      item.querySelector(".api-legend").setAttribute("aria-pressed", String(item === selected));
    }
  }

  for (const item of series) {
    const legend = item.querySelector(".api-legend");
    item.addEventListener("pointerenter", () => {
      hovered = item;
      render();
    });
    item.addEventListener("pointerleave", () => {
      if (hovered === item) hovered = null;
      render();
    });
    legend.addEventListener("focus", () => {
      hovered = item;
      render();
    });
    legend.addEventListener("blur", () => {
      if (hovered === item) hovered = null;
      render();
    });
    function toggleSelection() {
      selected = selected === item ? null : item;
      render();
    }
    legend.addEventListener("click", toggleSelection);
    legend.addEventListener("keydown", (event) => {
      if (event.key === "Enter" || event.key === " ") {
        event.preventDefault();
        toggleSelection();
      } else if (event.key === "Escape") {
        selected = null;
        render();
      }
    });
  }
}
